//! The connector half of `oxy-app`'s `project_ctx`: a configured database in,
//! a [`DatabaseConnector`] (or its [`ConnectorConfig`]) out. The free functions
//! that translate `config.yml` databases — `*_var` resolution, the DuckDB
//! local/S3-mirror arms, the Airhouse wire endpoint and credential mint — with
//! no dependency on `OxyProjectContext`, which calls them. Split out of
//! `project_ctx/mod.rs` and moved down here on 2026-11-22 (the decompose's
//! W1b), which broke the `project_ctx` ↔ `metric_tree_runner` cycle;
//! `oxy-app` re-exports the public three at
//! `agentic_wiring::project_ctx::{build_connector_for_db,
//! database_to_connector_config, try_database_to_connector_config}`.

use std::sync::Arc;

use agentic_connector::{
    BigQueryConfig, ClickHouseConfig, ConnectorConfig, DatabaseConnector, DomoConfig, DuckDbConfig,
    DuckDbLoadStrategy, DuckDbRawConfig, DuckDbUrlConfig, MysqlConfig, PostgresConfig,
    SnowflakeAuth, SnowflakeConfig,
};
use entity::workspace_members::WorkspaceRole;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::WorkingCopy;
use oxy::config::model::{BigQueryKey, DatabaseType, DuckDBOptions, SnowflakeAuthType};
use oxy::config::{ConfigManager, DiskSlot, ResolveWorkspaceFile};
use oxy_shared::errors::OxyError;

mod airhouse_mint;
mod read_only;
pub use airhouse_mint::CredentialUse;
use airhouse_mint::mint_airhouse_managed_creds;
pub(crate) use read_only::resolve_read_only_airhouse;

/// Build a connector for one database, on either capability.
///
/// The three arms that need a disk go through `try_resolve_file`
/// ([`ResolveWorkspaceFile`]), which has an impl for each capability: on a node
/// with a working copy it resolves; on one without it returns the error naming
/// the fix. Every other database — remote warehouses, DuckDB with an S3 mirror,
/// airhouse — never touches a file and works anywhere.
///
/// Inside a workspace-preview request (`oxy_app_core::write_hold`) the connector
/// is held: reads are sent, writes refused. A caller off the request's task
/// that must hold too (the metric-tree runner's blocking threads) wraps with
/// `write_hold::hold_if` itself.
pub async fn build_connector_for_db<S: DiskSlot>(
    workspace_manager: &WorkspaceManager<S>,
    db_name: &str,
    subject: Option<uuid::Uuid>,
    role: Option<WorkspaceRole>,
) -> Result<Arc<dyn DatabaseConnector>, OxyError>
where
    ConfigManager<S>: ResolveWorkspaceFile,
{
    let conn = build_unheld_connector(workspace_manager, db_name, subject, role).await?;
    let held = oxy_app_core::write_hold::active();
    let session = oxy_app_core::write_hold::connector::Session::of(
        &workspace_manager.config_manager,
        db_name,
    );
    Ok(oxy_app_core::write_hold::hold_if(
        held, conn, db_name, session,
    ))
}

/// Internal: bypasses the preview write hold; only `OxyProjectContext` may call.
/// Crate-private again since W4 moved the context down beside it, and pinned
/// to `project_ctx` inside the crate by `unheld_callers_guard`.
///
/// [`build_connector_for_db`] without the preview hold: never handed out
/// without asking whether the caller is a preview request.
pub(crate) async fn build_unheld_connector<S: DiskSlot>(
    workspace_manager: &WorkspaceManager<S>,
    db_name: &str,
    subject: Option<uuid::Uuid>,
    role: Option<WorkspaceRole>,
) -> Result<Arc<dyn DatabaseConnector>, OxyError>
where
    ConfigManager<S>: ResolveWorkspaceFile,
{
    let db = workspace_manager
        .config_manager
        .resolve_database(db_name)
        .map_err(|e| {
            OxyError::ConfigurationError(format!(
                "database '{db_name}' not found in config.yml: {e}"
            ))
        })?;

    // Airhouse types live outside `agentic-connector`'s vendor-neutral
    // dispatcher; the helper builds them directly with full error
    // propagation. Other types go through the config + dispatcher path.
    match &db.database_type {
        DatabaseType::Airhouse(_) | DatabaseType::AirhouseManaged(_) => {
            build_airhouse_connector(
                &db,
                workspace_manager,
                (subject, role),
                CredentialUse::SharedPool,
            )
            .await
        }
        // DuckDB Local: use the process-wide pool so CSV/Parquet files are
        // loaded only once and subsequent calls get a cheap try_clone().
        // This is the same pool that oxy-core's DuckDB connector uses,
        // avoiding the fresh-connection-per-request cost.
        // `s3_mirror.is_none()` is the whole point of this guard. A mirrored
        // DuckDB must NOT take the local-file path: it falls through to the
        // wildcard arm below, which delegates to `database_to_connector_config`,
        // whose own mirror arm attaches from S3 when the process is Serve or
        // Worker. Without this clause the `Local` pattern matched first and a
        // mirrored database on a diskless node resolved a file that is not
        // there — contradicting this function's own doc comment above, which
        // already promises that "DuckDB with an S3 mirror … never touches a
        // file and works anywhere".
        DatabaseType::DuckDB(duck)
            if duck.s3_mirror.is_none() && matches!(&duck.options, DuckDBOptions::Local { .. }) =>
        {
            let DuckDBOptions::Local { file_search_path } = &duck.options else {
                unreachable!()
            };
            let resolved = workspace_manager
                .config_manager
                .try_resolve_file(file_search_path)
                .await
                .map_err(|e| {
                    OxyError::ConfigurationError(format!(
                        "DuckDB '{db_name}': cannot resolve path '{file_search_path}': {e}"
                    ))
                })?;
            let conn = tokio::task::spawn_blocking(move || {
                oxy::connector::checkout_local_connection(&resolved)
            })
            .await
            .map_err(|e| OxyError::DBError(format!("DuckDB pool join error: {e}")))?
            .map_err(|e| OxyError::DBError(format!("DuckDB pool checkout failed: {e}")))?;
            Ok(Arc::new(agentic_connector::DuckDbConnector::new(conn))
                as Arc<dyn DatabaseConnector>)
        }
        _ => {
            let cfg = try_database_to_connector_config(&db, workspace_manager)
                .await?
                .ok_or_else(|| {
                    OxyError::ConfigurationError(format!(
                        "could not build connector config for database '{db_name}' (type {})",
                        db.database_type_name()
                    ))
                })?;
            let built = agentic_connector::build_connector_async(cfg)
                .await
                .map_err(|e| {
                    OxyError::DBError(format!("failed to build connector for '{db_name}': {e}"))
                })?;
            Ok(Arc::from(built))
        }
    }
}

/// The process-environment fallback for a secret the store did not answer —
/// never for a name the manager withholds (`SecretsManager::withholding`, the
/// workspace-preview platform), which would otherwise come back through here.
pub fn env_fallback(
    secrets: &oxy::adapters::secrets::SecretsManager,
    name: &str,
) -> Option<String> {
    if secrets.withholds(name) {
        return None;
    }
    std::env::var(name).ok()
}

// ── Connector translation ───────────────────────────────────────────────────

/// Take a resolved connection field, falling back to `unset_default` only when
/// the author set neither the field nor its `*_var`.
///
/// `SecretsManager::resolve_config_value` answers one of three ways, and they
/// are not the same kind of answer:
///
/// * `Ok(value)` — the literal, or the `*_var` resolved (workspace secret in
///   oxydb first, then the process environment).
/// * `ConfigurationError` — neither the field nor its `*_var` is set. The
///   default is legitimate here: an unset ClickHouse user or database means
///   "the server's default", and an unset Postgres host has always meant
///   `localhost` for a database on the developer's own machine.
/// * `SecretNotFound(var)` — the author NAMED where the value lives and it is
///   not there. That is an error, never a default. Defaulting is how a
///   ClickHouse `host_var` with no matching secret became a connection to
///   `http://localhost:8123` with `database = ''`: the query then failed as
///   "error sending request for url (http://localhost:8123/…)", naming a host
///   nobody configured, and the agent retried it until `max iterations
///   exceeded`. The secret's name appeared nowhere.
///
/// Any other error (the secrets store itself failing) is propagated as well:
/// a default there would hide an outage behind the same wrong host.
fn connection_field(
    db: &oxy::config::model::Database,
    field: &str,
    resolved: Result<String, OxyError>,
    unset_default: &str,
) -> Result<String, OxyError> {
    match resolved {
        Ok(v) => Ok(v),
        Err(OxyError::ConfigurationError(_)) => Ok(unset_default.to_string()),
        Err(OxyError::SecretNotFound(var)) => Err(unresolved_var_error(db, field, var.as_deref())),
        Err(e) => Err(OxyError::ConfigurationError(format!(
            "database '{}' ({}): could not resolve `{field}`: {e}",
            db.name,
            db.database_type_name()
        ))),
    }
}

/// The error for a declared `*_var` that resolved to nothing. Names the
/// database, the field, the secret and both places it was looked for, so the
/// person reading it can fix it without reading code.
fn unresolved_var_error(
    db: &oxy::config::model::Database,
    field: &str,
    var: Option<&str>,
) -> OxyError {
    let var = var.unwrap_or("<unnamed>");
    OxyError::ConfigurationError(format!(
        "database '{}' ({}): `{field}_var: {var}` did not resolve — no secret named {var} \
         is available to this workspace, and the process has no environment variable of \
         that name. Add {var} to the workspace's secrets.",
        db.name,
        db.database_type_name()
    ))
}

/// [`connection_field`] for a field with no usable default — a Snowflake
/// password, a BigQuery key path, a MotherDuck token. Unset is an error too.
fn required_field<T>(
    db: &oxy::config::model::Database,
    field: &str,
    resolved: Result<T, OxyError>,
) -> Result<T, OxyError> {
    match resolved {
        Ok(v) => Ok(v),
        Err(OxyError::SecretNotFound(var)) => Err(unresolved_var_error(db, field, var.as_deref())),
        Err(e) => Err(OxyError::ConfigurationError(format!(
            "database '{}' ({}): could not resolve `{field}`: {e}",
            db.name,
            db.database_type_name()
        ))),
    }
}

/// A port, by the [`connection_field`] rules, that must also be a number. A
/// `port_var` holding `abc` used to become the default port without a word.
fn connection_port(
    db: &oxy::config::model::Database,
    resolved: Result<String, OxyError>,
    unset_default: u16,
) -> Result<u16, OxyError> {
    let raw = connection_field(db, "port", resolved, &unset_default.to_string())?;
    raw.trim().parse().map_err(|_| {
        OxyError::ConfigurationError(format!(
            "database '{}' ({}): port `{raw}` is not a port number",
            db.name,
            db.database_type_name()
        ))
    })
}

/// Internal: bypasses the preview write hold; only `OxyProjectContext` may call.
/// Crate-private again since W4 moved the context down beside it, and pinned
/// to `project_ctx` inside the crate by `unheld_callers_guard`.
pub(crate) async fn resolve_connector_impl(
    db_name: &str,
    workspace_manager: &WorkspaceManager<WorkingCopy>,
) -> Option<ConnectorConfig> {
    let db = match workspace_manager.config_manager.resolve_database(db_name) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(
                db = %db_name,
                "databases: '{}' not found in config.yml: {}",
                db_name,
                e
            );
            return None;
        }
    };
    database_to_connector_config(&db, workspace_manager).await
}

/// Translate an already-resolved [`oxy::config::model::Database`] into an
/// [`agentic_connector::ConnectorConfig`].
///
/// This is the secret-resolving counterpart of [`ConnectorConfig`]'s
/// constructors: it reads the per-type host / port / user / password /
/// database_var / developer_token_var values off `workspace_manager`'s
/// `SecretsManager`, and for DuckDB + BigQuery also walks the
/// workspace-relative file paths through `config_manager.resolve_file`.
///
/// Returns `None` for database shapes that [`agentic-connector`] does not yet
/// handle — today, Snowflake browser-auth and Snowflake private-key auth
/// both fall into this bucket. The test-connection handler uses that as a
/// signal to fall back to the legacy `oxy::connector::Connector` path.
///
/// A caller that can report a failure should use
/// [`try_database_to_connector_config`], which says WHY a database could not
/// be translated (a `*_var` that names no secret) instead of returning `None`.
pub async fn database_to_connector_config<S: DiskSlot>(
    db: &oxy::config::model::Database,
    workspace_manager: &WorkspaceManager<S>,
) -> Option<ConnectorConfig>
where
    ConfigManager<S>: ResolveWorkspaceFile,
{
    match try_database_to_connector_config(db, workspace_manager).await {
        Ok(config) => config,
        Err(e) => {
            tracing::error!(db = %db.name, "cannot build a connector: {e}");
            None
        }
    }
}

/// [`database_to_connector_config`], with the reason a database could not be
/// translated.
///
/// `Ok(None)` keeps its meaning — a shape `agentic-connector` does not handle,
/// or a database another path builds (airhouse). `Err` is a connection field
/// that does not resolve: a declared `*_var` with no workspace secret and no
/// environment variable of that name, or a secrets store that could not be
/// read. Every process resolves the same way — the request middleware, the
/// queued-run builders and the preagg cycle all hand the manager a
/// `SecretsManager::from_database_with_env_fallback` for the workspace — so a
/// value that resolves on one role resolves on every role.
pub async fn try_database_to_connector_config<S: DiskSlot>(
    db: &oxy::config::model::Database,
    workspace_manager: &WorkspaceManager<S>,
) -> Result<Option<ConnectorConfig>, OxyError>
where
    ConfigManager<S>: ResolveWorkspaceFile,
{
    let sm = &workspace_manager.secrets_manager;
    match &db.database_type {
        // Stateless fleet only (Serve/Worker): the local DuckDB data file is
        // absent here. The compile worker mirrored this local-file warehouse to
        // S3 and recorded an `s3_mirror` block; attach from S3 via the same
        // httpfs SQL the core connector uses, instead of resolving the
        // (nonexistent) local path. (DuckLake / MotherDuck are already remote and
        // carry no mirror.)
        //
        // File DuckDB is deprecated and refused at compile on the cloud roles
        // (`compile-runtime/src/config_gate.rs`), so no new revision carries a mirror; this
        // arm serves the revisions compiled before that refusal until their
        // workspaces move to Airhouse, and goes with them.
        //
        // Gated on the stateless roles: a node WITH the working copy (ide /
        // all-in-one) reads the SAME compiled config (which carries `s3_mirror`)
        // but DOES have the files, so it must fall through to the `Local` arm
        // below. The S3-mirror connector exposes data as views only and has no
        // `file_search_path`, so it can't resolve authored file-path SQL
        // (`FROM 'oxymart.csv'`) — which workflow/automation steps emit and which
        // is exactly what runs on the ide once /agentic-workflows is ide-pinned.
        DatabaseType::DuckDB(duck)
            if duck.s3_mirror.is_some()
                && matches!(
                    oxy_app_core::process_role::current_process_role(),
                    oxy_shared::fleet_role::Role::Serve | oxy_shared::fleet_role::Role::Worker
                ) =>
        {
            let mirror = duck.s3_mirror.as_ref().expect("guarded by is_some()");
            Ok(Some(ConnectorConfig::DuckDbRaw(DuckDbRawConfig {
                init_statements: oxy::connector::build_s3_mirror_sql(mirror),
                // The pod's credential chain: the statements hold no secret.
                secret_values: Vec::new(),
            })))
        }
        DatabaseType::DuckDB(duck) => match &duck.options {
            DuckDBOptions::Local { file_search_path } => {
                let path = match workspace_manager
                    .config_manager
                    .try_resolve_file(file_search_path)
                    .await
                {
                    Ok(p) => std::path::PathBuf::from(p),
                    Err(e) => {
                        tracing::warn!(db = %db.name, "DuckDB: cannot resolve path: {e}");
                        return Ok(None);
                    }
                };
                Ok(Some(ConnectorConfig::DuckDb(DuckDbConfig {
                    data_dir: path,
                    load_strategy: DuckDbLoadStrategy::View,
                    reuse_cached_directory: true,
                })))
            }
            DuckDBOptions::File { path } => {
                let resolved = match workspace_manager
                    .config_manager
                    .try_resolve_file(path)
                    .await
                {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!(db = %db.name, "DuckDB file: cannot resolve path: {e}");
                        return Ok(None);
                    }
                };
                Ok(Some(ConnectorConfig::DuckDbUrl(DuckDbUrlConfig {
                    url: resolved,
                    // Read-only on the cloud roles: file DuckDB is deprecated there.
                    read_only: oxy::file_duckdb_limits::limits_apply(),
                })))
            }
            DuckDBOptions::DuckLake(ducklake_config) => {
                let attach = match ducklake_config
                    .to_duckdb_attach_stmt(&workspace_manager.secrets_manager)
                    .await
                {
                    Ok(s) => s,
                    // Its secrets (`ManagedSecret` S3 / catalog values) did not
                    // resolve: the reason, not a `None` the caller has to guess at.
                    Err(e) => {
                        return Err(OxyError::ConfigurationError(format!(
                            "database '{}' (duckdb, ducklake): cannot build the attach: {e}",
                            db.name
                        )));
                    }
                };
                Ok(Some(ConnectorConfig::DuckDbRaw(DuckDbRawConfig {
                    init_statements: attach.statements,
                    secret_values: attach.secret_values,
                })))
            }
        },

        DatabaseType::Postgres(pg) => {
            // The `get_*` defaults (`localhost`, `5432`, `postgres`) apply only
            // when neither the field nor its `*_var` is set; a declared var that
            // resolves to nothing is an error — see [`connection_field`].
            let host = connection_field(db, "host", pg.get_host(sm).await, "localhost")?;
            let port = connection_port(db, pg.get_port(sm).await, 5432)?;
            let user = connection_field(db, "user", pg.get_user(sm).await, "")?;
            let password = connection_field(db, "password", pg.get_password(sm).await, "")?;
            let database = connection_field(db, "database", pg.get_database(sm).await, "")?;
            Ok(Some(ConnectorConfig::Postgres(PostgresConfig {
                host,
                port,
                user,
                password,
                database,
                // An author-configured database; Oxy has no opinion on its TLS,
                // so this keeps tokio_postgres's `prefer`. `postgres_managed`
                // below is the one where Oxy DID decide.
                sslmode: None,
            })))
        }

        // `Airhouse` and `AirhouseManaged` are handled by the host's
        // `resolve_pre_built_connector` path — see `resolve_pre_built_airhouse`
        // below — because the connector lives in the standalone `airhouse`
        // crate, not in `agentic-connector::ConnectorConfig`.
        DatabaseType::Airhouse(_) | DatabaseType::AirhouseManaged(_) => Ok(None),

        // `postgres_managed` carries no connection details in config.yml —
        // they come from the org's `oltp_tenants` row, reached through
        // `workspace_manager.workspace_id`.
        //
        // Returning `None` here does NOT fall through to another builder, as
        // an earlier comment claimed: the caller turns `None` into
        // "could not build connector config", which is what broke schema
        // introspection in the IDE. Airhouse gets away with `None` only
        // because it has a genuine second path (`resolve_pre_built_airhouse`).
        //
        // Always the read-only analyst — see `oxy_oltp::resolver`.
        DatabaseType::PostgresManaged(_) => {
            let Ok(db_conn) = oxy::database::client::establish_connection().await else {
                return Ok(None);
            };
            match oxy_oltp::resolver::resolve_analyst_connection(
                &db_conn,
                workspace_manager.workspace_id,
            )
            .await
            {
                // TLS posture is carried, not dropped. The resolver computes it
                // per provider precisely so a managed tenant cannot fall back to
                // plaintext, and this struct was where that decision used to die
                // — leaving `postgres_managed` at `prefer`, i.e. TLS only if the
                // server insists.
                //
                // `require` alone only ENCRYPTS: the connector maps it to
                // `verify_tls = false` (a no-op cert verifier), so an active MITM
                // could present any certificate and harvest the analyst password
                // and every row. When the tenant demands verification we hand the
                // connector `verify-full` instead, which checks the chain against
                // `webpki_roots` — Neon's public cert is covered. Local stays on
                // `disable` (loopback, no certificate). The DSN string keeps
                // `c.sslmode` (`require`/`disable`), the only spellings libpq's
                // parser accepts; this stronger mode lives only on the connector.
                Ok(c) => Ok(Some(ConnectorConfig::Postgres(PostgresConfig {
                    host: c.host,
                    port: c.port,
                    user: c.user,
                    password: c.password,
                    database: c.database,
                    sslmode: Some(if c.verify_tls {
                        "verify-full".to_string()
                    } else {
                        c.sslmode
                    }),
                }))),
                Err(e) => {
                    tracing::warn!(
                        db = %db.name,
                        workspace_id = %workspace_manager.workspace_id,
                        "postgres_managed resolve failed: {e}"
                    );
                    Ok(None)
                }
            }
        }

        DatabaseType::Redshift(rds) => {
            let host = connection_field(db, "host", rds.get_host(sm).await, "localhost")?;
            let port = connection_port(db, rds.get_port(sm).await, 5439)?;
            let user = connection_field(db, "user", rds.get_user(sm).await, "")?;
            let password = connection_field(db, "password", rds.get_password(sm).await, "")?;
            let database = connection_field(db, "database", rds.get_database(sm).await, "")?;
            Ok(Some(ConnectorConfig::Redshift(PostgresConfig {
                host,
                port,
                user,
                password,
                database,
                sslmode: None,
            })))
        }

        DatabaseType::ClickHouse(ch) => {
            // An unset user, password or database means the server's default,
            // and an unset host keeps the historical `localhost`. A declared
            // `host_var` that resolves to nothing is an error, never
            // `http://localhost:8123` — see [`connection_field`].
            let host = connection_field(db, "host", ch.get_host(sm).await, "localhost")?;
            let user = connection_field(db, "user", ch.get_user(sm).await, "")?;
            let password = connection_field(db, "password", ch.get_password(sm).await, "")?;
            let database = connection_field(db, "database", ch.get_database(sm).await, "")?;
            let url = if host.contains("://") {
                // Caller already provided a full URL (scheme + host + optional
                // port + optional path).  Pass it through verbatim — common for
                // ClickHouse Cloud (HTTPS, port 8443) and port-forwarded hosts
                // where the user enters the full address.
                host
            } else if host.contains(':') {
                // Bare host:port — synthesize the scheme only.
                format!("http://{host}")
            } else {
                // Bare hostname — fall back to the default ClickHouse HTTP port.
                format!("http://{host}:8123")
            };
            Ok(Some(ConnectorConfig::ClickHouse(ClickHouseConfig {
                url,
                user,
                password,
                database,
            })))
        }

        DatabaseType::Snowflake(sf) => {
            let auth = match &sf.auth_type {
                SnowflakeAuthType::BrowserAuth {
                    browser_timeout_secs,
                    cache_dir,
                } => SnowflakeAuth::Browser {
                    timeout_secs: *browser_timeout_secs,
                    cache_dir: cache_dir.clone(),
                    sso_url_callback: None,
                },
                SnowflakeAuthType::PrivateKey { .. } => {
                    tracing::warn!(
                        db = %db.name,
                        "Snowflake: private-key-file auth is not supported in the agentic \
                         connector; hold the key in a secret with `private_key_var`"
                    );
                    return Ok(None);
                }
                SnowflakeAuthType::PrivateKeyVar { .. } => {
                    let pem = sf.get_private_key_from_secret(sm).await.map_err(|e| {
                        OxyError::ConfigurationError(format!(
                            "database '{}' (snowflake): cannot resolve `private_key_var`: {e}",
                            db.name
                        ))
                    })?;
                    SnowflakeAuth::PrivateKey {
                        private_key_pem: pem.expect("auth type is PrivateKeyVar"),
                    }
                }
                _ => {
                    let password = required_field(db, "password", sf.get_password(sm).await)?;
                    SnowflakeAuth::Password { password }
                }
            };
            Ok(Some(ConnectorConfig::Snowflake(SnowflakeConfig {
                account: sf.account.clone(),
                username: sf.username.clone(),
                auth,
                role: sf.role.clone(),
                warehouse: sf.warehouse.clone(),
                database: Some(sf.database.clone()),
                schema: sf.schema.clone(),
            })))
        }

        DatabaseType::Bigquery(bq) => {
            let key_field = if bq.key_json_var.is_some() {
                "key_json"
            } else {
                "key_path"
            };
            let (key_path, key_json) = match required_field(db, key_field, bq.get_key(sm).await)? {
                // The key's contents, from the secret store: nothing on disk.
                BigQueryKey::Json(json) => (String::new(), Some(json)),
                BigQueryKey::Path(key_path) => {
                    let key_path = workspace_manager
                        .config_manager
                        .try_resolve_file(&key_path)
                        .await
                        .unwrap_or(key_path);
                    (key_path, None)
                }
            };
            let project_id = match &key_json {
                Some(json) => BigQueryKey::Json(json.clone()).project_id(),
                None => BigQueryKey::Path(key_path.clone()).project_id(),
            }
            .unwrap_or_default();
            {
                // Merge legacy single `dataset` + multi `datasets` map keys.
                let mut datasets: Vec<String> = bq.datasets.keys().cloned().collect();
                if let Some(ref ds) = bq.dataset
                    && !datasets.contains(ds)
                {
                    datasets.push(ds.clone());
                }
                Ok(Some(ConnectorConfig::BigQuery(BigQueryConfig {
                    key_path,
                    key_json,
                    project_id,
                    datasets,
                })))
            }
        }

        DatabaseType::MotherDuck(md) => {
            let token = required_field(db, "token", md.get_token(sm).await)?;
            let url = match &md.database {
                Some(db) => format!("md:{}?motherduck_token={}", db, token),
                None => format!("md:?motherduck_token={}", token),
            };
            Ok(Some(ConnectorConfig::DuckDbUrl(DuckDbUrlConfig {
                url,
                read_only: false,
            })))
        }

        DatabaseType::Mysql(my) => {
            let host = connection_field(db, "host", my.get_host(sm).await, "localhost")?;
            let port = connection_port(db, my.get_port(sm).await, 3306)?;
            let user = connection_field(db, "user", my.get_user(sm).await, "")?;
            let password = connection_field(db, "password", my.get_password(sm).await, "")?;
            let database = connection_field(db, "database", my.get_database(sm).await, "")?;
            Ok(Some(ConnectorConfig::Mysql(MysqlConfig {
                host,
                port,
                user,
                password,
                database,
            })))
        }

        DatabaseType::DOMO(d) => {
            let developer_token = match sm.resolve_secret(&d.developer_token_var).await {
                Ok(Some(t)) => t,
                Ok(None) => {
                    return Err(unresolved_var_error(
                        db,
                        "developer_token",
                        Some(&d.developer_token_var),
                    ));
                }
                Err(e) => {
                    return Err(OxyError::ConfigurationError(format!(
                        "database '{}' (domo): could not resolve `developer_token`: {e}",
                        db.name
                    )));
                }
            };
            Ok(Some(ConnectorConfig::Domo(DomoConfig::from_instance(
                &d.instance,
                developer_token,
                &d.dataset_id,
            ))))
        }
    }
}

/// Pre-build an `airhouse::AirhouseConnector` for `airhouse` /
/// `airhouse_managed` databases. Errors are swallowed into a warn log and
/// returned as `None` because this is the agentic-pipeline path: a single
/// failing database shouldn't kill batch resolution. Non-agentic callers
/// (the SQL IDE, the builder bridge) use `OxyProjectContext::build_connector_for`
/// instead, which propagates the same errors so the user sees what's wrong.
///
/// Internal: bypasses the preview write hold; only `OxyProjectContext` may call.
/// Crate-private again since W4 moved the context down beside it, and pinned
/// to `project_ctx` inside the crate by `unheld_callers_guard`.
pub(crate) async fn resolve_pre_built_airhouse(
    db_name: &str,
    workspace_manager: &WorkspaceManager<WorkingCopy>,
    subject: Option<uuid::Uuid>,
    role: Option<WorkspaceRole>,
) -> Option<Arc<dyn DatabaseConnector>> {
    let db = workspace_manager
        .config_manager
        .resolve_database(db_name)
        .ok()?;
    if !matches!(
        db.database_type,
        DatabaseType::Airhouse(_) | DatabaseType::AirhouseManaged(_)
    ) {
        return None;
    }
    match build_airhouse_connector(
        &db,
        workspace_manager,
        (subject, role),
        CredentialUse::SharedPool,
    )
    .await
    {
        Ok(conn) => Some(conn),
        Err(e) => {
            tracing::warn!(db = %db.name, "airhouse connector build failed: {e}");
            None
        }
    }
}

/// Error-propagating airhouse connector builder. Used by both
/// [`resolve_pre_built_airhouse`] (which discards the error) and
/// `OxyProjectContext::build_connector_for` (which surfaces it). Keeps
/// the per-`DatabaseType` arms in one place.
///
/// The actual `AirhouseConnector` is **reused across requests** via
/// [`crate::airhouse_pool`] keyed by logical identity, so the
/// per-request `OxyProjectContext` no longer opens a fresh pgwire connection
/// (and a fresh server-side DuckLake session) per warehouse query. On a warm
/// hit no credential is minted and no connection is opened; the closure below
/// runs only on a miss or dead-connection rebuild.
pub(crate) async fn build_airhouse_connector<S: DiskSlot>(
    db: &oxy::config::model::Database,
    workspace_manager: &WorkspaceManager<S>,
    (subject, role): (Option<uuid::Uuid>, Option<WorkspaceRole>),
    credential: CredentialUse,
) -> Result<Arc<dyn DatabaseConnector>, OxyError> {
    let is_managed = matches!(db.database_type, DatabaseType::AirhouseManaged(_));
    let read_only = credential == CredentialUse::ReadOnly;
    if read_only && !is_managed {
        return Err(OxyError::ConfigurationError(format!(
            "{}: a static airhouse database has no read-only credential to mint",
            db.name
        )));
    }
    let key = match read_only {
        // Its own pool: a read-only connection never serves a role's queries.
        true => format!("mgd:{}:read-only", workspace_manager.workspace_id),
        false => airhouse_pool_key(
            is_managed,
            workspace_manager.workspace_id,
            subject,
            role.as_ref(),
            &db.name,
        ),
    };

    let pool_key = key.clone();
    let conn = crate::airhouse_pool::get_or_build(key, || async {
        // This credential goes into the pool below, so it is minted for the
        // pool rather than for whoever happens to open it.
        let AirhouseWireParams {
            mut host,
            mut port,
            user,
            password,
            database,
            expires_at,
        } = airhouse_wire_params(db, workspace_manager, subject, role.clone(), credential).await?;

        // Route query workloads — analytics agent, Data-App, SQL-IDE: everything
        // funneled through build_airhouse_connector — to the dedicated analytics
        // DP pool when one is configured, isolating heavy OLAP from the
        // latency-sensitive serving/ingest DP. airway ingest shares
        // `airhouse_wire_params` directly (not this path) and cameras has its own
        // client, so their writes stay on the main serving endpoint. Unset
        // (AIRHOUSE_ANALYTICS_WIRE_HOST) → keep the serving endpoint (no change).
        if let Some(ep) = airhouse::analytics_wire_endpoint()
            && (ep.host.as_str(), ep.port) != (host.as_str(), port) {
                tracing::debug!(
                    from = %format!("{host}:{port}"),
                    to = %format!("{}:{}", ep.host, ep.port),
                    "airhouse query connector routed to the analytics pool"
                );
                host = ep.host;
                port = ep.port;
            }

        // The connect is retried while airhouse answers `53300`, and takes a
        // slot in this process's airhouse connection budget which it holds
        // until the connection closes. The credential above is minted once and
        // reused across attempts — the whole retry budget is ~1.4s against a
        // 900s TTL. See `airhouse_pool::ceiling`.
        let conn = crate::airhouse_pool::ceiling::connect_within_budget(
            &pool_key,
            || async {
                airhouse::AirhouseConnector::new(&host, port, &user, &password, &database)
                    .await
                    .map_err(|e| {
                        OxyError::DBError(format!(
                            "airhouse connector failed to connect to {host}:{port}/{database} as {user}: {e}"
                        ))
                    })
            },
        )
        .await?;
        // Every requester at this key shares the connection and its DuckDB
        // session, so a statement that changes session state is refused
        // rather than handed on to the next one.
        let conn = conn.shared_session();
        // Tell the pool when airhouse will stop accepting this credential, so
        // it retires the connection first rather than being hung up on.
        let conn = match expires_at {
            Some(at) => conn.credential_expires_at(at),
            None => conn,
        };
        Ok(Arc::new(conn))
    })
    .await?;
    Ok(match read_only {
        true => Arc::new(crate::read_only_airhouse::ReadOnlyAirhouse::confirmed(conn)),
        false => conn,
    })
}

/// Logical pool identity for an airhouse connector. Deliberately *not* the
/// minted username: the broker re-mints a fresh ephemeral username roughly
/// every 15 min, but the live connection (and thus this key) must persist
/// across that rotation. `airhouse_managed` is per
/// `(workspace, user|system, role)`; static airhouse shares fixed creds so
/// it keys per `(workspace, db)`.
///
/// **Not per requester.** Keying this on the individual `subject` cost one pool
/// per user per role per process, so held connections grew with an org's
/// headcount rather than with its concurrency — the `identities` term in
/// `width × identities × processes`, and the reason the analytics plane's
/// pod-wide cap went 64 → 160 → 320 in seven days without settling
/// (`internal-docs/airhouse-connection-scaling.md`). It bought nothing to
/// offset that: airhouse resolves a username to a **role** and nothing else
/// (`rbac.rs::pick_role`), then authorizes on `(role, statement type)` plus a
/// tenant-level write scope, with no row-level or per-user filter anywhere on
/// the read path — its own result cache keys on `{tenant}:{sql}` for the same
/// reason. Two readers of one workspace at the same role therefore saw
/// byte-identical data and already shared cached results. `role` stays in the
/// key, so this grants exactly what the per-subject key granted.
///
/// **The credential follows the key.** Because one connection now serves every
/// requester at a role, it is minted for `SystemPurpose::SharedQuery` at that
/// role rather than for a user: a `mint_for_user` here would stamp whichever
/// requester opened the connection onto queries other people ran. Coarse and
/// true beats precise and false, and oxy's own audit trail still records the
/// individual.
fn airhouse_pool_key(
    is_managed: bool,
    workspace_id: uuid::Uuid,
    subject: Option<uuid::Uuid>,
    role: Option<&WorkspaceRole>,
    db_name: &str,
) -> String {
    if is_managed {
        // A request carrying a user is one bucket; a background path with no
        // subject keeps its own, so a scheduled run never shares a connection
        // with an interactive one.
        let subj = match subject {
            Some(_) => "user",
            None => "system",
        };
        let r = role
            .map(|r| format!("{r:?}"))
            .unwrap_or_else(|| "none".to_string());
        format!("mgd:{workspace_id}:{subj}:{r}")
    } else {
        format!("static:{workspace_id}:{db_name}")
    }
}

/// Build a `postgresql://` URL, percent-encoding the userinfo and path
/// so credentials containing `@ : / ?` survive parsing by airway's
/// Postgres/airhouse destination.
pub fn pg_wire_dsn(user: &str, password: &str, host: &str, port: u16, database: &str) -> String {
    format!(
        "postgresql://{}:{}@{}:{}/{}",
        urlencoding::encode(user),
        urlencoding::encode(password),
        host,
        port,
        urlencoding::encode(database),
    )
}

/// Resolve the Postgres-wire connection parameters for an `airhouse` /
/// `airhouse_managed` database. For `airhouse_managed` this mints an
/// ephemeral per-subject credential via the SA broker. Shared by the
/// connector builder and the airway destination resolver.
/// `sharing` is passed straight to [`mint_airhouse_managed_creds`] and the
/// caller must choose it deliberately — see [`CredentialUse`].
/// Everything needed to open an airhouse pgwire connection, plus **when the
/// credential stops working**.
///
/// `expires_at` is the field that matters and the easy one to drop: static
/// airhouse has no expiry, but an `airhouse_managed` credential is ephemeral,
/// and airhouse closes the socket the moment it lapses. A pool that never
/// learns the deadline can only find out by being hung up on mid-request. See
/// `airhouse_pool::expiry`.
pub struct AirhouseWireParams {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub database: String,
    /// Unix seconds, or `None` for a configured (non-expiring) credential.
    pub expires_at: Option<u64>,
}

/// Internal: bypasses the preview write hold (raw wire credentials); only
/// `OxyProjectContext` may call. Crate-private again since W4 moved the
/// context down beside it, and pinned to `project_ctx` inside the crate by
/// `unheld_callers_guard`.
pub(crate) async fn airhouse_wire_params<S>(
    db: &oxy::config::model::Database,
    workspace_manager: &WorkspaceManager<S>,
    subject: Option<uuid::Uuid>,
    role: Option<WorkspaceRole>,
    sharing: CredentialUse,
) -> Result<AirhouseWireParams, OxyError> {
    let params = match &db.database_type {
        DatabaseType::Airhouse(ah) => {
            // The local-airhouse defaults apply only to fields the author left
            // unset; a declared `*_var` with no secret is an error, as for every
            // other warehouse — see [`connection_field`].
            let sm = &workspace_manager.secrets_manager;
            let host = connection_field(db, "host", ah.get_host(sm).await, "localhost")?;
            let port = connection_port(db, ah.get_port(sm).await, 5445)?;
            let user = connection_field(db, "user", ah.get_user(sm).await, "admin")?;
            let password = connection_field(db, "password", ah.get_password(sm).await, "airhouse")?;
            let database = connection_field(db, "database", ah.get_database(sm).await, "airhouse")?;
            // Configured credential: nothing mints it, so nothing expires it.
            AirhouseWireParams {
                host,
                port,
                user,
                password,
                database,
                expires_at: None,
            }
        }
        DatabaseType::AirhouseManaged(_) => {
            mint_airhouse_managed_creds(workspace_manager, subject, role, sharing).await?
        }
        other => {
            return Err(OxyError::ConfigurationError(format!(
                "build_airhouse_connector: unexpected database_type {other:?}"
            )));
        }
    };

    Ok(params)
}

#[cfg(test)]
mod connector_translation_tests;

#[cfg(test)]
mod unheld_callers_guard;

#[cfg(test)]
mod tests {
    use super::{airhouse_pool_key, env_fallback};

    /// A withheld name does not come back through the environment fallback
    /// (the LLM `key_var` and `resolve_secret` paths); any other name does.
    #[test]
    fn env_fallback_skips_a_withheld_name() {
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::set_var("S9_FALLBACK_WITHHELD", "secret");
            std::env::set_var("S9_FALLBACK_ORDINARY", "fine");
        }
        let sm = oxy::adapters::secrets::SecretsManager::from_environment()
            .unwrap()
            .withholding(["S9_FALLBACK_WITHHELD".to_string()].into());
        assert_eq!(env_fallback(&sm, "S9_FALLBACK_WITHHELD"), None);
        assert_eq!(
            env_fallback(&sm, "S9_FALLBACK_ORDINARY").as_deref(),
            Some("fine")
        );
    }

    #[test]
    fn airhouse_pool_key_is_stable_per_identity() {
        let ws = uuid::Uuid::nil();
        let user = uuid::Uuid::from_u128(1);
        let role = entity::workspace_members::WorkspaceRole::Admin;

        // Managed: keyed by (workspace, user|system, role) — NOT the minted
        // username, so it survives credential rotation.
        let k1 = airhouse_pool_key(true, ws, Some(user), Some(&role), "wh");
        let k2 = airhouse_pool_key(true, ws, Some(user), Some(&role), "wh");
        assert_eq!(k1, k2);
        assert!(k1.starts_with("mgd:"));

        // Two users of one workspace at the same role SHARE a pool. This is
        // the point of the key, not an accident: airhouse resolves a username
        // to a role and nothing else, then authorizes on (role, statement
        // type) plus a tenant-level write scope, so these two sessions are
        // granted the same thing and already share airhouse's own
        // {tenant}:{sql} result cache. Keying them apart grew held
        // connections with headcount instead of concurrency.
        let other_user =
            airhouse_pool_key(true, ws, Some(uuid::Uuid::from_u128(2)), Some(&role), "wh");
        assert_eq!(k1, other_user);
        // And no user id reaches the key, so it cannot grow per person.
        assert!(!k1.contains(&user.to_string()));

        // The boundaries that remain, each of which IS a difference in what
        // airhouse grants or in what the connection is for.
        let other_role = airhouse_pool_key(
            true,
            ws,
            Some(user),
            Some(&entity::workspace_members::WorkspaceRole::Member),
            "wh",
        );
        let no_role = airhouse_pool_key(true, ws, Some(user), None, "wh");
        let system = airhouse_pool_key(true, ws, None, None, "wh");
        let other_ws = airhouse_pool_key(
            true,
            uuid::Uuid::from_u128(9),
            Some(user),
            Some(&role),
            "wh",
        );
        assert_ne!(k1, other_role, "a different role is a different authority");
        assert_ne!(k1, no_role, "no role is not the same as a role");
        assert_ne!(
            k1, system,
            "a background path must not share with an interactive one"
        );
        assert_ne!(k1, other_ws, "never across workspaces");
        assert!(system.contains("system"));

        // Static airhouse keys by (workspace, db) and ignores subject/role.
        let s1 = airhouse_pool_key(false, ws, Some(user), Some(&role), "wh");
        let s2 = airhouse_pool_key(false, ws, None, None, "wh");
        assert_eq!(s1, s2);
        assert_eq!(s1, format!("static:{ws}:wh"));
    }

    /// A role change moves a requester to a different key, which is why
    /// sharing a connection per role is not a revocation hole.
    ///
    /// The effective role is resolved per request (`EffectiveWorkspaceRole`,
    /// set on the request's extensions by the workspace middleware), so a
    /// demoted or removed member's *next* request keys on their new role and
    /// gets a credential minted at it. The entry for the old role stays
    /// reachable only by someone whose current role is that role, which is
    /// correct by definition; it then idles out. This held for the old
    /// per-user key too — nothing evicted it on demotion either — so the
    /// window is unchanged. If targeted eviction is ever wanted, the key to
    /// drop is `mgd:<ws>:user:<old_role>`, and there is now one entry per role
    /// rather than one per person to find.
    #[test]
    fn a_role_change_moves_the_requester_to_another_key() {
        use entity::workspace_members::WorkspaceRole;
        let ws = uuid::Uuid::nil();
        let user = uuid::Uuid::from_u128(7);

        let as_admin = airhouse_pool_key(true, ws, Some(user), Some(&WorkspaceRole::Admin), "wh");
        let as_viewer = airhouse_pool_key(true, ws, Some(user), Some(&WorkspaceRole::Viewer), "wh");
        assert_ne!(
            as_admin, as_viewer,
            "a demotion must not land on the old role's pooled connection"
        );
        // And the demoted requester lands exactly where every other Viewer of
        // this workspace lands — no private entry carrying stale authority.
        let another_viewer = airhouse_pool_key(
            true,
            ws,
            Some(uuid::Uuid::from_u128(8)),
            Some(&WorkspaceRole::Viewer),
            "wh",
        );
        assert_eq!(as_viewer, another_viewer);
    }

    /// The identity count per workspace per process is now bounded by the role
    /// enum rather than by headcount — the whole point of the change.
    ///
    /// `width × identities × processes` is what fills the analytics plane's
    /// pod-wide connection cap, and `identities` was the unbounded term: a
    /// 40-person org could hold 40 pools per role per process. This pins the
    /// bound so a future key change cannot quietly reintroduce it.
    #[test]
    fn airhouse_pool_identities_are_bounded_by_role_not_headcount() {
        use entity::workspace_members::WorkspaceRole;
        let ws = uuid::Uuid::nil();
        let roles = [
            WorkspaceRole::Owner,
            WorkspaceRole::Admin,
            WorkspaceRole::Member,
            WorkspaceRole::Viewer,
        ];

        let mut keys = std::collections::BTreeSet::new();
        // Forty people, every role, plus the role-less and background paths.
        for n in 0..40u128 {
            let user = uuid::Uuid::from_u128(n + 1);
            for role in &roles {
                keys.insert(airhouse_pool_key(true, ws, Some(user), Some(role), "wh"));
            }
            keys.insert(airhouse_pool_key(true, ws, Some(user), None, "wh"));
        }
        keys.insert(airhouse_pool_key(true, ws, None, None, "wh"));

        // roles + "none" for a user, + system. Independent of the 40.
        assert_eq!(
            keys.len(),
            roles.len() + 2,
            "identities per workspace per process must not grow with headcount: {keys:?}"
        );
    }
}
