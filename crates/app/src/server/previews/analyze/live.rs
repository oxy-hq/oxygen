//! What production has today, for the change check to compare against: the
//! stored schema Airway keeps per pipeline (Oxy's Postgres), and the columns
//! the live tables actually have (the workspace's Airhouse, read through a
//! Reader credential minted for `SystemPurpose::Preview`).

use std::collections::HashSet;
use std::time::Duration;

use agentic_airway::extension::workspace_pipeline_state;
use agentic_airway::schema_compat::{LiveColumn, Schema};
use async_trait::async_trait;
use sea_orm::{DatabaseConnection, DbErr, EntityTrait};
use tokio::sync::OnceCell;
use tokio_postgres::SimpleQueryMessage;
use uuid::Uuid;

/// Bound on opening Airhouse (credential mint plus wire connect), and on each
/// column read. Generous against a cold DuckDB session; past it the pipeline's
/// drift is `Unevaluated`.
const AIRHOUSE_TIMEOUT: Duration = Duration::from_secs(30);

/// The columns under one live dataset. A trait so the check can be driven
/// without an Airhouse; production uses [`AirhouseLiveTables`].
#[async_trait]
pub trait LiveTables: Send + Sync {
    async fn columns(&self, dataset: &str) -> Result<Vec<LiveColumn>, String>;
}

/// The workspace's Airhouse, opened on first use — so a branch whose changed
/// pipelines never need a live read never mints a credential — and reused for
/// every dataset after. A failed open is remembered: the check reports it once
/// per pipeline rather than retrying the connection for each.
pub struct AirhouseLiveTables {
    workspace_id: Uuid,
    client: OnceCell<Result<tokio_postgres::Client, String>>,
}

impl AirhouseLiveTables {
    pub fn new(workspace_id: Uuid) -> Self {
        Self {
            workspace_id,
            client: OnceCell::new(),
        }
    }

    /// The mint and the wire connect, bounded together. Neither bounds itself:
    /// the Airhouse admin client has no request timeout, and
    /// `Config::connect_timeout` covers only TCP — a handshake HAProxy holds
    /// open stalls until its server timeout (see
    /// `oxy_cameras::airhouse::connect_timeout`). Unbounded, either leaves the
    /// check `pending` for as long as it stalls.
    async fn connect(&self) -> Result<tokio_postgres::Client, String> {
        tokio::time::timeout(AIRHOUSE_TIMEOUT, self.connect_unbounded())
            .await
            .map_err(|_| format!("opening Airhouse timed out after {AIRHOUSE_TIMEOUT:?}"))?
    }

    async fn connect_unbounded(&self) -> Result<tokio_postgres::Client, String> {
        // Reads belong on the analytics pool when there is one, as every other
        // Airhouse SELECT path routes them.
        let endpoint = airhouse::analytics_wire_endpoint()
            .or_else(airhouse::wire_endpoint)
            .ok_or("Airhouse is not configured on this deployment")?;
        let broker =
            airhouse::token_broker().ok_or("the Airhouse token broker is not initialised")?;
        let cred = broker
            .mint_for_system(
                self.workspace_id,
                airhouse::SystemPurpose::Preview,
                airhouse::UserRole::Reader,
                airhouse::DEFAULT_INTERNAL_TTL,
            )
            .await
            .map_err(|e| format!("minting a Reader credential: {e}"))?;
        let mut config = tokio_postgres::Config::new();
        config
            .host(&endpoint.host)
            .port(endpoint.port)
            .user(&cred.username)
            .password(&cred.password)
            .dbname(&cred.tenant);
        // Plaintext on the private network, as every Airhouse wire client is
        // (see `airhouse::connector`).
        let (client, connection) = config
            .connect(tokio_postgres::NoTls)
            .await
            .map_err(|e| format!("connecting to Airhouse: {e}"))?;
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                tracing::debug!("previews: live-table connection ended: {e}");
            }
        });
        Ok(client)
    }
}

#[async_trait]
impl LiveTables for AirhouseLiveTables {
    async fn columns(&self, dataset: &str) -> Result<Vec<LiveColumn>, String> {
        let client = self
            .client
            .get_or_init(|| self.connect())
            .await
            .as_ref()
            .map_err(Clone::clone)?;
        // Simple protocol, as `airhouse::connector` uses: Airhouse's extended
        // protocol metadata does not round-trip through tokio-postgres. The
        // dataset is a spec field, so it is quoted as a literal, never spliced.
        let sql = format!(
            "SELECT table_name, column_name, data_type, is_nullable \
             FROM information_schema.columns WHERE table_schema = {} \
             ORDER BY table_name, ordinal_position",
            quote_literal(dataset)
        );
        let messages = tokio::time::timeout(AIRHOUSE_TIMEOUT, client.simple_query(&sql))
            .await
            .map_err(|_| {
                format!("reading information_schema.columns timed out after {AIRHOUSE_TIMEOUT:?}")
            })?
            .map_err(|e| format!("reading information_schema.columns: {e}"))?;
        Ok(messages.iter().filter_map(live_column).collect())
    }
}

fn live_column(msg: &SimpleQueryMessage) -> Option<LiveColumn> {
    let SimpleQueryMessage::Row(row) = msg else {
        return None;
    };
    Some(LiveColumn {
        table: row.get("table_name")?.to_string(),
        column: row.get("column_name")?.to_string(),
        data_type: row.get("data_type").unwrap_or_default().to_string(),
        nullable: row
            .get("is_nullable")
            .is_some_and(|v| v.eq_ignore_ascii_case("yes")),
    })
}

fn quote_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Production's stored schema for the live pipeline `name`, or `None` when it
/// never loaded (or a reset left a tombstone). A stored schema that will not
/// deserialize is also `None`, with a warning: the check then compares against
/// the live spec alone, which can only under-report a reset, not invent one.
pub(super) async fn stored_schema(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    name: &str,
) -> Result<Option<Schema>, DbErr> {
    let row = workspace_pipeline_state::Entity::find_by_id((workspace_id, name.to_string()))
        .one(db)
        .await?;
    let Some(json) = row.and_then(|r| r.schema_json) else {
        return Ok(None);
    };
    Ok(serde_json::from_value(json)
        .map_err(|e| {
            tracing::warn!(%workspace_id, pipeline = name, error = %e,
                "previews: stored airway schema does not deserialize; checking without it");
        })
        .ok())
}

/// The `config.yml` databases of `revision` that are the workspace's own
/// managed Airhouse — the only destinations whose live tables the check can
/// read with a workspace credential.
pub(super) async fn managed_airhouse_databases(
    db: &DatabaseConnection,
    revision: Uuid,
) -> Result<HashSet<String>, DbErr> {
    let row = entity::workspace_compiled_configs::Entity::find_by_id(revision)
        .one(db)
        .await?;
    let databases = row.map(|r| r.databases).unwrap_or_default();
    Ok(databases
        .as_array()
        .into_iter()
        .flatten()
        .filter(|d| d.get("type").and_then(|t| t.as_str()) == Some("airhouse_managed"))
        .filter_map(|d| d.get("name").and_then(|n| n.as_str()).map(str::to_string))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dataset_is_quoted_as_a_literal() {
        assert_eq!(quote_literal("raw"), "'raw'");
        assert_eq!(
            quote_literal("x'; DROP TABLE t; --"),
            "'x''; DROP TABLE t; --'"
        );
    }
}
