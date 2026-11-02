use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use clickhouse::ClickHouse;
use connectorx::ConnectorX;
pub use domo::DOMO;
use duckdb::DuckDB;
use engine::Engine;
use motherduck::MotherDuck;
use snowflake::Snowflake;
use std::collections::HashMap;

use crate::config::ResolveWorkspaceFile;
use crate::{
    adapters::{
        secrets::SecretsManager,
        session_filters::{FilterProcessor, SessionFilters},
    },
    config::{
        ConfigManager,
        model::{
            BigQueryKey, ConnectionOverride, ConnectionOverrides, Database, DatabaseType,
            DuckDBOptions,
        },
    },
};
use oxy_shared::errors::OxyError;

mod clickhouse;
pub mod connection_string;
mod connectorx;
mod constants;
mod domo;
mod duckdb;
pub use duckdb::{
    build_s3_mirror_sql, checkout_file_connection, checkout_local_connection,
    release_local_connection,
};
mod duckdb_pool;
mod engine;
pub mod managed;
mod motherduck;
mod snowflake;
mod utils;

pub use connection_string::{
    ConnectionStringError, ConnectionStringFormatter, ConnectionStringParser,
    PostgresConnectionString,
};
pub use managed::{
    ManagedConnectionResolver, managed_connection_resolver, set_managed_connection_resolver,
};
pub use utils::{load_result, write_to_ipc};

#[enum_dispatch::enum_dispatch(Engine)]
#[derive(Debug)]
enum EngineType {
    DuckDB,
    ConnectorX,
    ClickHouse,
    Snowflake,
    DOMO,
    MotherDuck,
}

#[derive(Debug)]
pub struct Connector {
    engine: EngineType,
}

/// Return a friendly error if `database_ref` resolves to an
/// `airhouse_managed` database. Use this from system-side entry points
/// (schema inspection, connection-test, CLI runs) that don't carry user
/// or workspace context — without this guard those callers hit the
/// verbose `Connector::from_db` ConfigurationError instead.
///
/// Non-airhouse_managed refs return `Ok(())` and the caller proceeds
/// normally. An unknown `database_ref` also returns `Ok(())` so the
/// caller's own resolver gets to produce the canonical "not found"
/// error.
pub fn reject_airhouse_managed_for_system_path<S>(
    config_manager: &ConfigManager<S>,
    database_ref: &str,
    operation: &str,
) -> Result<(), OxyError> {
    let Ok(db) = config_manager.resolve_database(database_ref) else {
        return Ok(());
    };
    if matches!(db.database_type, DatabaseType::AirhouseManaged(_)) {
        return Err(OxyError::ConfigurationError(format!(
            "{operation} of airhouse_managed databases is not supported here — \
             this entry point doesn't carry the per-user identity that the \
             credential broker needs. Run the equivalent action inside an \
             `oxy serve` session (the IDE Database panel and agent runs \
             both work)."
        )));
    }
    Ok(())
}

impl Connector {
    pub async fn from_database<S>(
        database_ref: &str,
        config_manager: &ConfigManager<S>,
        secrets_manager: &SecretsManager,
        dry_run_limit: Option<u64>,
        filters: Option<SessionFilters>,
        connections: Option<ConnectionOverrides>,
        subject: Option<uuid::Uuid>,
        workspace_id: Option<uuid::Uuid>,
        effective_role: Option<entity::workspace_members::WorkspaceRole>,
    ) -> Result<Self, OxyError>
    where
        ConfigManager<S>: ResolveWorkspaceFile,
    {
        let database = config_manager.resolve_database(database_ref)?;
        Self::from_db(
            &database,
            config_manager,
            secrets_manager,
            dry_run_limit,
            filters,
            connections.and_then(|c| c.get(database_ref).cloned()),
            None, // No SSO URL sender for regular operations
            subject,
            workspace_id,
            effective_role,
        )
        .await
    }

    /// Build a `Connector` from a fully-resolved [`Database`] config.
    ///
    /// `subject` is the oxy user id for whom the connector is being built.
    /// `workspace_id` is the workspace whose airhouse tenant the broker
    /// should mint against. Both are required for `airhouse_managed`; if
    /// either is `None` and the database resolves to `airhouse_managed`,
    /// the call returns a `ConfigurationError` explaining what was missing.
    /// All other backends ignore them.
    ///
    /// `effective_role` is the user's resolved workspace role. Only
    /// consulted by the `airhouse_managed` arm: the
    /// [`ManagedConnectionResolver`](managed::ManagedConnectionResolver) picks
    /// the airhouse role for the minted credential from it
    /// (Owner→Admin, Admin→Writer, Member/Viewer→Reader). When `None`,
    /// the arm conservatively defaults to **Reader** — meaning any DDL/DML
    /// (`INSERT` / `UPDATE` / `CREATE` / `DROP`) issued through this
    /// connector will fail with a permission-denied at the database, even
    /// for an Owner. Threading the real role from the caller's run context is
    /// the supported way to grant write access to Automation /
    /// agent SQL steps; the IDE Database panel does this automatically
    /// via `OxyProjectContext::build_connector_for`.
    pub async fn from_db<S>(
        database: &Database,
        config_manager: &ConfigManager<S>,
        secrets_manager: &SecretsManager,
        dry_run_limit: Option<u64>,
        filters: Option<SessionFilters>,
        connections: Option<ConnectionOverride>,
        sso_url_sender: Option<tokio::sync::mpsc::Sender<String>>,
        subject: Option<uuid::Uuid>,
        workspace_id: Option<uuid::Uuid>,
        effective_role: Option<entity::workspace_members::WorkspaceRole>,
    ) -> Result<Self, OxyError>
    where
        ConfigManager<S>: ResolveWorkspaceFile,
    {
        let engine = match &database.database_type {
            DatabaseType::Bigquery(bigquery) => {
                let dry_run_limit = dry_run_limit.or(bigquery.dry_run_limit);
                match bigquery.get_key(secrets_manager).await? {
                    // The key's contents, from the secret store: no file anywhere.
                    BigQueryKey::Json(key_json) => EngineType::ConnectorX(
                        ConnectorX::new(database.dialect(), String::new(), dry_run_limit)
                            .with_bigquery_key_json(key_json),
                    ),
                    BigQueryKey::Path(key_path_str) => {
                        let key_path = if bigquery.key_path.is_some() {
                            config_manager.try_resolve_file(&key_path_str).await?
                        } else {
                            key_path_str
                        };
                        tracing::debug!(key_path = %key_path, "BigQuery key path resolved");
                        EngineType::ConnectorX(ConnectorX::new(
                            database.dialect(),
                            key_path,
                            dry_run_limit,
                        ))
                    }
                }
            }
            DatabaseType::DuckDB(duckdb) => {
                // When an S3 mirror is present we're on a stateless replica: the
                // local path doesn't exist, so skip resolving it (the connector
                // reads from S3) and thread the mirror through.
                let mirror = duckdb.s3_mirror.clone();
                match &duckdb.options {
                    DuckDBOptions::Local { file_search_path } => {
                        let options = if mirror.is_some() {
                            DuckDBOptions::Local {
                                file_search_path: file_search_path.clone(),
                            }
                        } else {
                            DuckDBOptions::Local {
                                file_search_path: config_manager
                                    .try_resolve_file(file_search_path)
                                    .await?,
                            }
                        };
                        EngineType::DuckDB(DuckDB::new(options, mirror, secrets_manager.clone()))
                    }
                    DuckDBOptions::File { path } => {
                        let options = if mirror.is_some() {
                            DuckDBOptions::File { path: path.clone() }
                        } else {
                            DuckDBOptions::File {
                                path: config_manager.try_resolve_file(path).await?,
                            }
                        };
                        EngineType::DuckDB(DuckDB::new(options, mirror, secrets_manager.clone()))
                    }
                    DuckDBOptions::DuckLake(config) => EngineType::DuckDB(DuckDB::new(
                        DuckDBOptions::DuckLake(config.clone()),
                        mirror,
                        secrets_manager.clone(),
                    )),
                }
            }
            DatabaseType::Postgres(pg) => {
                let db_path = format!(
                    "{}:{}@{}:{}/{}",
                    pg.get_user(secrets_manager).await?,
                    pg.get_password(secrets_manager).await?,
                    pg.get_host(secrets_manager).await?,
                    pg.get_port(secrets_manager).await?,
                    pg.get_database(secrets_manager).await?,
                );
                EngineType::ConnectorX(ConnectorX::new(database.dialect(), db_path, None))
            }
            DatabaseType::Airhouse(ah) => {
                // Airhouse speaks the Postgres wire protocol but its SQL dialect is
                // DuckDB, and the wire impl does not implement Postgres `COPY`.
                // ConnectorX's default `binary` protocol for Postgres uses
                // `COPY ... TO STDOUT` for bulk transfer, which Airhouse rejects
                // with an `UnexpectedMessage` protocol error. Force the `cursor`
                // protocol so ConnectorX issues plain `DECLARE CURSOR` + `FETCH`
                // statements — same mitigation as the Redshift path below.
                let db_path = format!(
                    "{}:{}@{}:{}/{}?cxprotocol=cursor",
                    ah.get_user(secrets_manager).await?,
                    ah.get_password(secrets_manager).await?,
                    ah.get_host(secrets_manager).await?,
                    ah.get_port(secrets_manager).await?,
                    ah.get_database(secrets_manager).await?,
                );
                EngineType::ConnectorX(ConnectorX::new("postgres".to_string(), db_path, None))
            }
            DatabaseType::AirhouseManaged(_) => {
                // `airhouse_managed` mints a fresh ephemeral credential for
                // every connector build. The mint needs `(workspace_id,
                // subject)` to key the cache and pick the tenant; if either is
                // missing we can't mint, so refuse with a typed error rather
                // than silently falling back to a less-privileged path.
                //
                // `effective_role` selects the airhouse role the credential is
                // minted at (Owner/Admin → write-capable, Member/Viewer →
                // Reader); `None` is least-privilege Reader, so write-capable
                // Automation steps must populate it — see the doc on `from_db`.
                // The role mapping, mint and DSN formatting live behind the
                // `ManagedConnectionResolver` port (oxy-app owns `airhouse`);
                // core only wraps the resolved DSN in a ConnectorX.
                let workspace_id = workspace_id.ok_or_else(|| {
                    OxyError::ConfigurationError(
                        "airhouse_managed requires a workspace context; no workspace_id was \
                         threaded into Connector::from_db. This typically means the caller \
                         needs to pass `Some(execution_context.workspace.workspace_id)`."
                            .into(),
                    )
                })?;
                let subject = subject.ok_or_else(|| {
                    OxyError::ConfigurationError(
                        "airhouse_managed requires a user identity; no subject (oxy user id) \
                         was threaded into Connector::from_db. This typically means the caller \
                         needs to pass `execution_context.user_id` — agent / automation runs are \
                         expected to populate it."
                            .into(),
                    )
                })?;
                let resolver = managed::managed_connection_resolver().ok_or_else(|| {
                    OxyError::ConfigurationError(
                        "airhouse_managed: no managed-connection resolver is registered; the \
                         airhouse integration must be wired at startup (oxy-app registers one; \
                         this entry point ran without it)."
                            .into(),
                    )
                })?;
                let db_path = resolver
                    .resolve_airhouse_managed(workspace_id, subject, effective_role)
                    .await?;
                EngineType::ConnectorX(ConnectorX::new("postgres".to_string(), db_path, None))
            }
            DatabaseType::PostgresManaged(_) => {
                // Resolves the per-org OLTP database for the caller's workspace
                // as a read-only analyst. Deliberately ignores `effective_role`
                // (unlike `airhouse_managed`): a per-org OLTP database holds a
                // customer's live business records, so an ad-hoc UPDATE typed
                // into the SQL IDE would be a catastrophe, not a feature. Writes
                // reach OLTP only through published code — an app function or an
                // Airway pipeline (`OltpProvisioner::ensure_writer`) — never
                // this path. The analyst lookup and DSN formatting (percent-
                // encoding, sslmode) live behind the `ManagedConnectionResolver`
                // port (oxy-app owns `oxy-oltp`); core only wraps the DSN.
                let workspace_id = workspace_id.ok_or_else(|| {
                    OxyError::ConfigurationError(
                        "postgres_managed requires a workspace context; no workspace_id was \
                         threaded into Connector::from_db. This typically means the caller \
                         needs to pass `Some(execution_context.workspace.workspace_id)`."
                            .into(),
                    )
                })?;
                let resolver = managed::managed_connection_resolver().ok_or_else(|| {
                    OxyError::ConfigurationError(
                        "postgres_managed: no managed-connection resolver is registered; the \
                         oxy-oltp integration must be wired at startup (oxy-app registers one)."
                            .into(),
                    )
                })?;
                let db_path = resolver.resolve_postgres_managed(workspace_id).await?;
                EngineType::ConnectorX(ConnectorX::new("postgres".to_string(), db_path, None))
            }
            DatabaseType::Redshift(rs) => {
                let db_path = format!(
                    "{}:{}@{}:{}/{}?cxprotocol={}",
                    rs.get_user(secrets_manager).await?,
                    rs.get_password(secrets_manager).await?,
                    rs.get_host(secrets_manager).await?,
                    rs.get_port(secrets_manager).await?,
                    rs.get_database(secrets_manager).await?,
                    // https://github.com/sfu-db/connector-x/blob/534617477f78b092ba169c71e64778b86d5853ad/connectorx-python/connectorx/__init__.py#L50-L66
                    // redshift only supports cursor protocol
                    "cursor"
                );
                EngineType::ConnectorX(ConnectorX::new(database.dialect(), db_path, None))
            }
            DatabaseType::Mysql(my) => {
                let db_path = format!(
                    "{}:{}@{}:{}/{}",
                    my.get_user(secrets_manager).await?,
                    my.get_password(secrets_manager).await?,
                    my.get_host(secrets_manager).await?,
                    my.get_port(secrets_manager).await?,
                    my.get_database(secrets_manager).await?,
                );
                EngineType::ConnectorX(ConnectorX::new(database.dialect(), db_path, None))
            }
            DatabaseType::ClickHouse(ch) => {
                let validated_filters = Self::validate_filters(&ch.filters, filters)?;

                let mut clickhouse_connector = ClickHouse::new(ch.clone(), secrets_manager.clone());
                if let Some(filters) = validated_filters {
                    clickhouse_connector = clickhouse_connector.with_filters(filters);
                }
                clickhouse_connector = clickhouse_connector.with_overrides(connections)?;
                EngineType::ClickHouse(clickhouse_connector)
            }
            DatabaseType::Snowflake(snowflake) => {
                let validated_filters = Self::validate_filters(&snowflake.filters, filters)?;

                let mut snowflake_connector = Snowflake::new(
                    snowflake.clone(),
                    secrets_manager.clone(),
                    config_manager.workspace_file_resolver(),
                );
                if let Some(filters) = validated_filters {
                    snowflake_connector = snowflake_connector.with_filters(filters);
                }
                snowflake_connector = snowflake_connector.with_overrides(connections)?;

                // Set SSO URL sender if provided
                if let Some(sender) = sso_url_sender {
                    snowflake_connector = snowflake_connector.with_sso_url_sender(sender);
                }

                EngineType::Snowflake(snowflake_connector)
            }
            DatabaseType::DOMO(domo) => {
                EngineType::DOMO(DOMO::from_config(secrets_manager.clone(), domo.clone()).await?)
            }
            DatabaseType::MotherDuck(motherduck) => EngineType::MotherDuck(
                MotherDuck::from_config(secrets_manager.clone(), motherduck.clone()).await?,
            ),
        };
        Ok(Connector { engine })
    }

    pub async fn run_query(&self, query: &str) -> Result<String, OxyError> {
        self.engine.run_query(query).await
    }

    pub async fn run_query_with_limit(
        &self,
        query: &str,
        dry_run_limit: Option<u64>,
    ) -> Result<(Vec<RecordBatch>, SchemaRef), OxyError> {
        self.engine.run_query_with_limit(query, dry_run_limit).await
    }

    pub async fn run_query_and_load(
        &self,
        query: &str,
    ) -> Result<(Vec<RecordBatch>, SchemaRef), OxyError> {
        self.engine.run_query_and_load(query).await
    }

    pub async fn explain_query(
        &self,
        query: &str,
    ) -> Result<(Vec<RecordBatch>, SchemaRef), OxyError> {
        self.engine.explain_query(query).await
    }

    pub async fn dry_run(&self, query: &str) -> Result<(Vec<RecordBatch>, SchemaRef), OxyError> {
        self.engine.dry_run(query).await
    }

    /// Validate api request filters against configured database filter schemas
    fn validate_filters(
        schemas: &HashMap<String, schemars::schema::SchemaObject>,
        filters: Option<SessionFilters>,
    ) -> Result<Option<SessionFilters>, OxyError> {
        let Some(filters) = filters else {
            // Log when no filters provided (may be required for some databases)
            if !schemas.is_empty() {
                tracing::debug!(
                    configured_filters = ?schemas.keys().collect::<Vec<_>>(),
                    "No filters provided for database with filter schema"
                );
            }
            return Ok(None);
        };

        if schemas.is_empty() {
            // Security event: filters provided but not configured
            tracing::warn!(
                provided_filters = ?filters.keys().collect::<Vec<_>>(),
                "Filters provided for database but no filter schema configured - ignoring filters"
            );
            return Ok(None);
        }

        // Log filter validation attempt for audit trail
        tracing::info!(
            provided_filters = ?filters.keys().collect::<Vec<_>>(),
            configured_filters = ?schemas.keys().collect::<Vec<_>>(),
            "Validating filters for database query"
        );

        let processor = FilterProcessor::new(schemas.clone());
        let validated = processor.process_filters(filters).map_err(|e| {
            // Log filter validation failure as security event
            tracing::error!(
                error = %e,
                "Filter validation failed - rejecting request"
            );
            e
        })?;

        // Log successful filter validation for audit trail
        tracing::info!(
            validated_filters = ?validated.keys().collect::<Vec<_>>(),
            "Filter validation successful - applying filters to query"
        );

        Ok(Some(validated))
    }
}
