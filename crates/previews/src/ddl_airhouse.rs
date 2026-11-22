//! The preview DDL port on the workspace's Airhouse: a system Writer
//! (`SystemPurpose::PreviewDdl`) that sends only the fixed statements
//! [`Ddl`] builds.
//!
//! Why a system Writer: a preview's own write credential is scoped to its
//! schemas, and whether Airhouse lets a scoped Writer create or drop a schema
//! is not settled (phase 2 plan, open question 2). Schema DDL therefore runs
//! here, on a credential that could do more, which is why every statement is
//! built from a name [`super::ddl::well_formed`] accepts and, for a drop, that a
//! registry row claimed by the drop run vouches for.
//!
//! TODO(previews S5): this is a tenant-wide Writer. Once S5 settles whether
//! Airhouse honours `write_schemas` on a Writer mint (and whether a scoped
//! Writer may create and drop its own schema), mint this for the one schema
//! (or a `preview_<key>__` prefix scope) instead.

use std::time::Duration;

use airhouse::preview_sql::PreviewNamespace;
use async_trait::async_trait;
use tokio::sync::Mutex;
use tokio_postgres::SimpleQueryMessage;
use uuid::Uuid;

use super::ddl::{
    Ddl, OpenSchemaDropper, PreviewDdlError, Relation, RelationKind, SchemaCreator, SchemaDropper,
    create_failed, list_relations_sql, schema_exists_sql,
};

/// Bound on opening Airhouse (mint plus wire connect) and on each statement.
/// Neither bounds itself; see `previews::analyze::live` for why.
const AIRHOUSE_TIMEOUT: Duration = Duration::from_secs(30);

enum Conn {
    Unopened,
    Open(tokio_postgres::Client),
    /// Opening failed, or a statement timed out and the connection was
    /// dropped. Every later statement fails fast with this.
    Broken(String),
}

/// One preview's schema DDL on the workspace's Airhouse, connected on first use
/// and reused for every statement after. Statements run one at a time.
pub struct AirhousePreviewDdl {
    workspace_id: Uuid,
    ns: PreviewNamespace,
    conn: Mutex<Conn>,
}

impl AirhousePreviewDdl {
    pub fn new(workspace_id: Uuid, ns: PreviewNamespace) -> Self {
        Self {
            workspace_id,
            ns,
            conn: Mutex::new(Conn::Unopened),
        }
    }

    /// Simple protocol, as `airhouse::connector` uses: Airhouse's extended
    /// protocol metadata does not round-trip through tokio-postgres.
    ///
    /// A statement that times out may still be running on the server, so its
    /// connection is dropped rather than reused, and every statement after it
    /// fails fast instead of queueing behind it.
    async fn query(&self, sql: &str) -> Result<Vec<SimpleQueryMessage>, PreviewDdlError> {
        let mut conn = self.conn.lock().await;
        if matches!(*conn, Conn::Unopened) {
            *conn = match tokio::time::timeout(AIRHOUSE_TIMEOUT, connect(self.workspace_id)).await {
                Ok(Ok(client)) => Conn::Open(client),
                Ok(Err(e)) => Conn::Broken(e),
                Err(_) => Conn::Broken(format!(
                    "opening Airhouse timed out after {AIRHOUSE_TIMEOUT:?}"
                )),
            };
        }
        let client = match &*conn {
            Conn::Open(client) => client,
            Conn::Broken(why) => return Err(PreviewDdlError::Backend(why.clone())),
            Conn::Unopened => return Err(PreviewDdlError::Backend("Airhouse not opened".into())),
        };
        let result = tokio::time::timeout(AIRHOUSE_TIMEOUT, client.simple_query(sql)).await;
        match result {
            Ok(rows) => rows.map_err(|e| PreviewDdlError::Backend(format!("{sql}: {e}"))),
            Err(_) => {
                let why = format!(
                    "{sql}: timed out after {AIRHOUSE_TIMEOUT:?}; the Airhouse connection was \
                     dropped and the statements after it were not sent"
                );
                *conn = Conn::Broken(why.clone());
                Err(PreviewDdlError::Backend(why))
            }
        }
    }

    async fn execute(&self, ddl: Ddl) -> Result<(), PreviewDdlError> {
        let sql = ddl.statement(&self.ns)?;
        self.query(&sql).await.map(|_| ())
    }
}

/// A system Writer on the workspace's primary Airhouse endpoint: DDL is a
/// write, so never the analytics pool.
async fn connect(workspace_id: Uuid) -> Result<tokio_postgres::Client, String> {
    let endpoint =
        airhouse::wire_endpoint().ok_or("Airhouse is not configured on this deployment")?;
    let broker = airhouse::token_broker().ok_or("the Airhouse token broker is not initialised")?;
    let cred = broker
        .mint_for_system(
            workspace_id,
            airhouse::SystemPurpose::PreviewDdl,
            airhouse::UserRole::Writer,
            airhouse::DEFAULT_INTERNAL_TTL,
        )
        .await
        .map_err(|e| format!("minting a Writer credential: {e}"))?;
    let mut config = tokio_postgres::Config::new();
    config
        .host(&endpoint.host)
        .port(endpoint.port)
        .user(&cred.username)
        .password(&cred.password)
        .dbname(&cred.tenant);
    // Plaintext on the private network, as every Airhouse wire client is.
    let (client, connection) = config
        .connect(tokio_postgres::NoTls)
        .await
        .map_err(|e| format!("connecting to Airhouse: {e}"))?;
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            tracing::debug!("previews: schema DDL connection ended: {e}");
        }
    });
    Ok(client)
}

fn first_count(rows: &[SimpleQueryMessage]) -> i64 {
    rows.iter()
        .find_map(|msg| match msg {
            SimpleQueryMessage::Row(row) => row.get("n")?.parse().ok(),
            _ => None,
        })
        .unwrap_or(0)
}

#[async_trait]
impl SchemaCreator for AirhousePreviewDdl {
    async fn create_schema(&self, schema: &str) -> Result<(), PreviewDdlError> {
        let Err(error) = self.execute(Ddl::CreateSchema(schema.to_string())).await else {
            return Ok(());
        };
        let PreviewDdlError::Backend(message) = error else {
            return Err(error);
        };
        let probe = schema_exists_sql(&self.ns, schema)?;
        let exists = first_count(&self.query(&probe).await?) > 0;
        Err(create_failed(schema, exists, message))
    }
}

#[async_trait]
impl SchemaDropper for AirhousePreviewDdl {
    async fn list_relations(&self, schema: &str) -> Result<Vec<Relation>, PreviewDdlError> {
        let sql = list_relations_sql(&self.ns, schema)?;
        let rows = self.query(&sql).await?;
        Ok(rows
            .iter()
            .filter_map(|msg| match msg {
                SimpleQueryMessage::Row(row) => Some(Relation {
                    name: row.get("table_name")?.to_string(),
                    kind: RelationKind::from_table_type(row.get("table_type").unwrap_or_default()),
                }),
                _ => None,
            })
            .collect())
    }

    async fn drop_relation(
        &self,
        schema: &str,
        relation: &Relation,
    ) -> Result<(), PreviewDdlError> {
        self.execute(Ddl::DropRelation(schema.to_string(), relation.clone()))
            .await
    }

    async fn drop_schema(&self, schema: &str) -> Result<(), PreviewDdlError> {
        self.execute(Ddl::DropSchema(schema.to_string())).await
    }
}

/// Opens [`AirhousePreviewDdl`] per preview: what the drop executor uses in
/// production.
pub struct AirhouseDroppers;

impl OpenSchemaDropper for AirhouseDroppers {
    fn open(&self, workspace_id: Uuid, ns: &PreviewNamespace) -> Box<dyn SchemaDropper> {
        Box::new(AirhousePreviewDdl::new(workspace_id, ns.clone()))
    }
}

/// One fixed drop statement on a fresh system Writer: `DROP SCHEMA` or `DROP
/// TABLE|VIEW` for one of `ns`'s schemas. For several statements, hold an
/// [`AirhousePreviewDdl`] instead so they share a connection.
///
/// `Ddl::CreateSchema` is refused here: a preview schema is created only
/// through `previews::registry::ensure_schema`, which writes its registry row
/// first and records that the preview created it. A schema made any other way
/// would have no such record, and the TTL drop would never take it.
pub async fn system_ddl(
    workspace_id: Uuid,
    ns: &PreviewNamespace,
    ddl: Ddl,
) -> Result<(), PreviewDdlError> {
    if let Ddl::CreateSchema(schema) = &ddl {
        return Err(PreviewDdlError::Refused(airhouse::preview_sql::Refused(
            format!(
                "{schema:?}: create preview schemas through previews::registry::ensure_schema, \
                 which registers them first"
            ),
        )));
    }
    AirhousePreviewDdl::new(workspace_id, ns.clone())
        .execute(ddl)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No Airhouse is configured in unit tests, so reaching the backend would
    /// fail differently: the refusal comes first, from the statement kind.
    #[tokio::test]
    async fn system_ddl_refuses_to_create_a_schema() {
        let ns = PreviewNamespace::from_key("feat_je_v2_92a1b7").unwrap();
        let err = system_ddl(
            Uuid::nil(),
            &ns,
            Ddl::CreateSchema("preview_feat_je_v2_92a1b7__toast_pos".into()),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, PreviewDdlError::Refused(_)), "{err:?}");
    }
}
