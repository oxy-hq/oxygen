use crate::agentic_wiring::OxyProjectContext;
use crate::agentic_wiring::project_ctx::database_to_connector_config;
use crate::server::api::middlewares::role_guards::WorkspaceAdmin;
use crate::server::api::middlewares::workspace_context::{
    EffectiveWorkspaceRole, WorkspaceManagerReadOnly, WorkspaceManagerWorkingCopy, WorkspacePath,
};
use crate::{
    cli::commands::clean::{clean_all, clean_cache, clean_database_folder, clean_vectors},
    server::service::{
        project::{
            database_config::DatabaseConfigBuilder,
            models::{WarehouseConfig, WarehousesFormData},
        },
        sync::{SyncFilter, sync_databases},
    },
};
use agentic_connector::{ConnectorConfig, SnowflakeAuth, SsoUrlCallback};
use axum::{
    extract::{Json, Path, Query},
    http::StatusCode,
    response::{IntoResponse, Response, sse::Sse},
};
use oxy::config::ResolveWorkspaceFile;
use oxy::config::model::{DatabaseType, SemanticModels, SnowflakeAuthType};
use oxy::connector::Connector;
use oxy::semantic::SemanticManager;
use oxy::semantic::inspector::{
    InspectEvent, InspectionResult, SchemaListResult, SchemaTablesResult, inspect_database,
    inspect_schema_tables, inspect_schemas,
};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use scopeguard::guard;
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use tokio::sync::mpsc;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct DatabaseInfo {
    pub name: String,
    /// SQL dialect used for query execution (e.g. `"duckdb"`, `"postgres"`).
    pub dialect: String,
    /// Raw config type from `config.yml` (e.g. `"airhouse_managed"`, `"duckdb"`).
    /// Use this for display (icons, labels) — `dialect` is for query engine selection.
    pub db_type: String,
    /// `None` when this instance could not look — the semantic sync directory
    /// lives in the working copy, so a replica has no view of it. `Some({})`
    /// means it looked and the database has no synced datasets. The UI says
    /// "No specific datasets configured" for the second, which is a statement
    /// about the customer's setup that only the first shape may not make.
    pub datasets: Option<HashMap<String, HashMap<String, SemanticModels>>>,
    pub synced: bool,
}

#[derive(Serialize, ToSchema)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
}

#[derive(Serialize, ToSchema)]
pub struct TableInfo {
    pub name: String,
    pub columns: Vec<ColumnInfo>,
}

#[derive(Serialize, ToSchema)]
pub struct DatabaseSchemaResponse {
    pub tables: Vec<TableInfo>,
}

#[derive(Deserialize)]
pub struct DatabaseSchemaPath {
    pub workspace_id: uuid::Uuid,
    pub database_name: String,
}

pub async fn get_database_schema(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Path(DatabaseSchemaPath {
        workspace_id: _,
        database_name,
    }): Path<DatabaseSchemaPath>,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    EffectiveWorkspaceRole(role): EffectiveWorkspaceRole,
) -> Result<Json<DatabaseSchemaResponse>, StatusCode> {
    // `build_connector_for_db` dispatches both the config + `agentic-connector`
    // path AND the host-built path (where airhouse lives), so this handler
    // works uniformly for every database type. Resolution / build failures
    // surface a 500 with the actual error logged — no more 422 swallowing.
    //
    // The free function rather than `OxyProjectContext::build_connector_for`:
    // the context is pinned to `<WorkingCopy>` and genericizing it would touch 26
    // construction sites, while this handler needs no disk. A database that
    // does — a local DuckDB file, a BigQuery key — still says so, through
    // `try_resolve_file`.
    let connector = crate::agentic_wiring::project_ctx::build_connector_for_db(
        &workspace_manager,
        &database_name,
        Some(user.id),
        Some(role),
    )
    .await
    .map_err(|e| {
        tracing::error!(
            "Failed to build connector for schema introspection of {}: {}",
            database_name,
            e
        );
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Let lazy connectors (e.g. Postgres) open their connection and pre-fetch
    // schema before the synchronous introspect_schema() call below.
    connector.prepare_schema().await.map_err(|e| {
        tracing::error!("Schema preparation failed for {}: {}", database_name, e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let schema = connector.introspect_schema().map_err(|e| {
        tracing::error!("Schema introspection failed for {}: {}", database_name, e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let tables = schema
        .tables
        .into_iter()
        .map(|t| TableInfo {
            name: t.name,
            columns: t
                .columns
                .into_iter()
                .map(|c| ColumnInfo {
                    name: c.name,
                    data_type: c.data_type,
                })
                .collect(),
        })
        .collect();

    Ok(Json(DatabaseSchemaResponse { tables }))
}

#[derive(Serialize, ToSchema)]
pub struct DatabaseSyncResponse {
    pub success: bool,
    pub message: String,
    pub sync_time_secs: Option<f64>,
}

// support deserializing datasets as either a single string or a list of strings
/// Deserialize a query param that can be:
/// - absent → None
/// - a single string → Some(vec![string]) (also splits on commas)
/// - a JSON array of strings → Some(vec![...])
fn deserialize_string_list<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<serde_json::Value>::deserialize(deserializer)?;
    match opt {
        None => Ok(None),
        Some(serde_json::Value::String(s)) => {
            // Support comma-separated values in a single string
            let items: Vec<String> = s
                .split(',')
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .collect();
            if items.is_empty() {
                Ok(None)
            } else {
                Ok(Some(items))
            }
        }
        Some(serde_json::Value::Array(arr)) => {
            let mut result = Vec::with_capacity(arr.len());
            for v in arr {
                match v {
                    serde_json::Value::String(s) => result.push(s),
                    _ => return Err(de::Error::custom("Expected string in array")),
                }
            }
            Ok(Some(result))
        }
        _ => Err(de::Error::custom("Invalid type for string list param")),
    }
}

#[derive(Deserialize, ToSchema)]
pub struct SyncDatabaseQuery {
    pub database: Option<String>,
    #[serde(default, deserialize_with = "deserialize_string_list")]
    pub datasets: Option<Vec<String>>,
    /// Specific tables to sync in "schema.table" format.
    /// When provided, only these tables are synced (overrides datasets).
    #[serde(default, deserialize_with = "deserialize_string_list")]
    pub tables: Option<Vec<String>>,
}

pub async fn sync_database(
    _: WorkspaceAdmin,
    WorkspaceManagerWorkingCopy(workspace_manager): WorkspaceManagerWorkingCopy,
    Path(WorkspacePath {
        workspace_id: _workspace_id,
    }): Path<WorkspacePath>,
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
    Query(params): Query<SyncDatabaseQuery>,
) -> Result<Json<DatabaseSyncResponse>, StatusCode> {
    let filter = if params.database.is_some() || params.tables.is_some() {
        Some(SyncFilter {
            database: params.database,
            datasets: params.datasets.unwrap_or_default(),
            tables: params.tables.unwrap_or_default(),
        })
    } else {
        None
    };

    let overwrite = true; // Always overwrite

    let config = workspace_manager.config_manager;
    let secrets_manager = workspace_manager.secrets_manager;

    match sync_databases(config.clone(), secrets_manager.clone(), filter, overwrite).await {
        Ok(results) => {
            let success_count = results.iter().filter(|r| r.is_ok()).count();
            let error_count = results.iter().filter(|r| r.is_err()).count();

            // Calculate average sync time from successful results
            let total_sync_time: f64 = results
                .iter()
                .filter_map(|result| match result {
                    Ok(sync_metrics) => Some(sync_metrics.sync_time_secs),
                    Err(_) => None,
                })
                .sum();

            let avg_sync_time = if success_count > 0 {
                Some(total_sync_time / success_count as f64)
            } else {
                None
            };

            let error_messages: Vec<String> = results
                .iter()
                .filter_map(|result| match result {
                    Err(e) => Some(e.to_string()),
                    Ok(_) => None,
                })
                .collect();

            let message = if error_count == 0 {
                if success_count == 1 {
                    "Database synced successfully".to_string()
                } else {
                    format!("{success_count} databases synced successfully")
                }
            } else if success_count == 0 {
                format!("Failed to sync: {}", error_messages.join("; "))
            } else {
                format!(
                    "{success_count} databases synced, {error_count} failed: {}",
                    error_messages.join("; ")
                )
            };

            Ok(Json(DatabaseSyncResponse {
                success: error_count == 0,
                message,
                sync_time_secs: avg_sync_time,
            }))
        }
        Err(e) => {
            tracing::error!("Database sync failed: {}", e);
            Ok(Json(DatabaseSyncResponse {
                success: false,
                message: format!("Database sync failed: {e}"),
                sync_time_secs: None,
            }))
        }
    }
}

pub async fn list_databases(
    // The listing is an in-memory `Config` read; only `datasets` needs the
    // working copy, because `SemanticStorage` resolves
    // `database_semantic_path()` under the workspace root. So the handler asks
    // for the files rather than requiring them, and the three answers this
    // endpoint already distinguishes — synced / not synced / could not look —
    // carry the difference the whole way to the UI (`DatasetInfo` renders the
    // third as "Not available on this instance", never as "none configured").
    //
    // Staying FleetOk matters: `useWorkspaceReadiness` calls this on every page
    // load and reads `databases.length` alone. Pinning it to the ide would put
    // the launcher behind the singleton for a field it never touches.
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
) -> Result<Json<Vec<DatabaseInfo>>, StatusCode> {
    let config_manager = &workspace_manager.config_manager;
    let secrets_manager = &workspace_manager.secrets_manager;

    // `Some` on a node that owns the workspace files, `None` on one that does
    // not. Not an error either way — "could not look" is one of this
    // endpoint's three answers.
    let semantic_manager = match config_manager.workspace_file_resolver() {
        Some(with_files) => Some(
            SemanticManager::from_config(with_files, secrets_manager.clone(), false)
                .await
                .map_err(|e| {
                    tracing::error!("Failed to create semantic manager: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR
                })?,
        ),
        None => None,
    };

    let mut databases = Vec::new();

    for db in config_manager.list_databases() {
        let Some(semantic_manager) = semantic_manager.as_ref() else {
            // No files on this node, so whether the database is synced is
            // unknown. Same answer `load_datasets` gives when it can see the
            // root is missing — reached without asking, because there is
            // nothing to ask.
            databases.push(DatabaseInfo {
                name: db.name.clone(),
                dialect: db.dialect(),
                db_type: db.database_type.to_string(),
                datasets: None,
                synced: false,
            });
            continue;
        };
        // Try to load cached database info (without triggering sync)
        let (datasets, synced) = match semantic_manager
            .try_load_cached_database_info(&db.name)
            .await
        {
            Ok(Some(db_info)) => {
                let datasets: HashMap<String, HashMap<String, SemanticModels>> = db_info
                    .datasets
                    .into_iter()
                    .map(|(dataset_name, dataset_info)| {
                        let tables: HashMap<String, SemanticModels> = dataset_info
                            .semantic_info
                            .iter()
                            .map(|(table_name, yaml_str)| {
                                let info = serde_yaml::from_str::<SemanticModels>(yaml_str)
                                    .unwrap_or_else(|e| {
                                        tracing::warn!(
                                            "Failed to parse semantic info for table '{}' in dataset '{}': {}, using default",
                                            table_name,
                                            dataset_name,
                                            e
                                        );
                                        // Fallback to default SemanticModels with basic info
                                        SemanticModels {
                                            table: table_name.clone(),
                                            database: db_info.name.clone(),
                                            description: String::new(),
                                            entities: Vec::new(),
                                            dimensions: Vec::new(),
                                            measures: Vec::new(),
                                            database_name: db_info.name.clone(),
                                        }
                                    });
                                (table_name.clone(), info)
                            })
                            .collect();
                        (dataset_name, tables)
                    })
                    .collect();
                (Some(datasets), true)
            }
            Ok(None) => {
                // Genuinely not synced yet: the sync root is here and this
                // database has nothing under it.
                (Some(HashMap::new()), false)
            }
            // Both shapes answer "unknown", and `datasets: None` is how that
            // is carried — `datasets: {}` would render as "No specific datasets
            // configured", a claim about the customer's setup made by a node
            // that never looked. `ConfigurationError` is how
            // `SemanticFileStorage::load_datasets` reports an absent sync root,
            // so it is worth naming in the log; it does not change the answer.
            Err(e) => {
                // Same answer either way; only the volume differs. The known
                // shape is routine on a replica and would be noise at `warn`;
                // anything else is a real fault, and merging both into `debug`
                // would have made it invisible at production log levels — the
                // loudness regression this branch exists to remove.
                if matches!(e, oxy_shared::errors::OxyError::ConfigurationError(_)) {
                    tracing::debug!(
                        database = %db.name,
                        error = ?e,
                        "list_databases: sync root not on this node; datasets unknown"
                    );
                } else {
                    tracing::warn!(
                        database = %db.name,
                        error = ?e,
                        "list_databases: dataset enrichment failed; datasets unknown"
                    );
                }
                (None, false)
            }
        };

        databases.push(DatabaseInfo {
            name: db.name.clone(),
            dialect: db.dialect(),
            db_type: db.database_type.to_string(),
            datasets,
            synced,
        });
    }

    Ok(Json(databases))
}

#[derive(Debug, Deserialize)]
pub struct CleanRequest {
    target: Option<CleanTarget>, // "all", "DatabasesFolder", "Vectors", "Cache"
                                 // DatabasesFolder: semantic models and build artifacts
                                 // Vectors: LanceDB embeddings and search indexes
                                 // Cache: temporary files, logs, and chart cache
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanTarget {
    All,
    DatabasesFolder,
    Vectors,
    Cache,
}
#[derive(Debug, Serialize)]
pub struct CleanResponse {
    success: bool,
    message: String,
    cleaned_items: Vec<String>,
}

pub async fn clean_data(
    _: WorkspaceAdmin,
    WorkspaceManagerWorkingCopy(workspace_manager): WorkspaceManagerWorkingCopy,
    Query(params): Query<CleanRequest>,
) -> Result<Json<CleanResponse>, StatusCode> {
    let target = params.target.unwrap_or(CleanTarget::All);

    let mut cleaned_items = Vec::new();
    let mut success = true;
    let mut error_message = String::new();

    match target {
        CleanTarget::All => match clean_all(false, &workspace_manager.config_manager).await {
            Ok(_) => {
                cleaned_items.extend(vec![
                    "Databases folder".to_string(),
                    "Vector store".to_string(),
                    "Cache".to_string(),
                ]);
            }
            Err(e) => {
                success = false;
                error_message = format!("Failed to clean all: {e}");
            }
        },
        CleanTarget::DatabasesFolder => {
            match clean_database_folder(false, &workspace_manager.config_manager).await {
                Ok(_) => cleaned_items.push("Databases folder".to_string()),
                Err(e) => {
                    success = false;
                    error_message = format!("Failed to clean databases folder: {e}");
                }
            }
        }
        CleanTarget::Vectors => match clean_vectors(false, &workspace_manager.config_manager).await
        {
            Ok(_) => cleaned_items.push("Vector store".to_string()),
            Err(e) => {
                success = false;
                error_message = format!("Failed to clean vectors: {e}");
            }
        },
        CleanTarget::Cache => match clean_cache(false, &workspace_manager.config_manager).await {
            Ok(_) => cleaned_items.push("Cache".to_string()),
            Err(e) => {
                success = false;
                error_message = format!("Failed to clean cache: {e}");
            }
        },
    }

    if success {
        Ok(Json(CleanResponse {
            success: true,
            message: format!("Successfully cleaned: {}", cleaned_items.join(", ")),
            cleaned_items,
        }))
    } else {
        tracing::error!("{}", error_message);
        Err(StatusCode::INTERNAL_SERVER_ERROR)
    }
}

#[derive(Serialize, ToSchema)]
pub struct CreateDatabaseConfigResponse {
    pub success: bool,
    pub message: String,
    pub databases_added: Vec<String>,
}

/// Creates database configurations and updates the config.yml file
#[utoipa::path(
    post,
    path = "/workspaces/{workspace_id}/databases",
    request_body = WarehousesFormData,
    params(
        ("workspace_id" = Uuid, Path, description = "Workspace ID")
    ),
    responses(
        (status = 201, description = "Database configurations created successfully", body = CreateDatabaseConfigResponse),
        (status = 400, description = "Bad request - validation failed"),
        (status = 409, description = "Conflict - database with same name already exists"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("ApiKey" = [])
    ),
    tag = "Databases"
)]
pub async fn create_database_config(
    _: WorkspaceAdmin,
    WorkspaceManagerWorkingCopy(workspace_manager): WorkspaceManagerWorkingCopy,
    Path(WorkspacePath { workspace_id: _ }): Path<WorkspacePath>,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Json(warehouses_form): Json<WarehousesFormData>,
) -> Result<Response, StatusCode> {
    let repo_path = workspace_manager.config_manager.workspace_path();

    tracing::info!(
        "Creating database configurations {:?}",
        warehouses_form.warehouses
    );
    let databases = DatabaseConfigBuilder::build_configs(
        &warehouses_form,
        repo_path,
        user.id,
        &workspace_manager.secrets_manager,
    )
    .await?;

    let database_names: Vec<String> = databases.iter().map(|db| db.name.clone()).collect();

    // Defense in depth: this handler mutates `config.yml`. If the route is ever
    // misclassified `FleetOk` and lands on a stateless replica, fail loudly here
    // rather than write to an ephemeral disk nobody else will read.
    crate::server::role_manifest::ensure_fs_writable("add databases to config.yml").map_err(
        |e| {
            tracing::error!(error = %e, "add_databases refused");
            StatusCode::INTERNAL_SERVER_ERROR
        },
    )?;

    // Add databases to the config and write to config.yml
    match workspace_manager
        .config_manager
        .add_databases(databases)
        .await
    {
        Ok(_) => {
            let response = CreateDatabaseConfigResponse {
                success: true,
                message: format!(
                    "{} database configuration(s) created successfully",
                    database_names.len()
                ),
                databases_added: database_names,
            };
            Ok((StatusCode::CREATED, Json(response)).into_response())
        }
        Err(e) => {
            tracing::error!("Failed to add databases to config: {}", e);

            // Check if it's a duplicate database error
            if e.to_string().contains("already exists") {
                Ok((
                    StatusCode::CONFLICT,
                    Json(json!({
                        "success": false,
                        "error": e.to_string()
                    })),
                )
                    .into_response())
            } else {
                Ok((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "success": false,
                        "error": format!("Failed to update configuration: {}", e)
                    })),
                )
                    .into_response())
            }
        }
    }
}

#[derive(Deserialize, ToSchema)]
pub struct TestDatabaseConnectionRequest {
    pub warehouse: WarehouseConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TestDatabaseConnectionResponse {
    pub success: bool,
    pub message: String,
    pub connection_time_ms: Option<u64>,
    pub error_details: Option<String>,
}

/// Connection test event types for SSE streaming
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConnectionTestEvent {
    Progress {
        message: String,
    },
    BrowserAuthRequired {
        sso_url: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        timeout_secs: Option<u64>,
    },
    Complete {
        result: TestDatabaseConnectionResponse,
    },
}

/// Test a database connection with real-time progress via SSE
#[utoipa::path(
    post,
    path = "/workspaces/{workspace_id}/databases/test-connection",
    request_body = TestDatabaseConnectionRequest,
    params(
        ("workspace_id" = uuid::Uuid, Path, description = "Workspace ID")
    ),
    responses(
        (status = 200, description = "Connection test stream", content_type = "text/event-stream"),
    ),
    security(
        ("ApiKey" = [])
    ),
    tag = "Databases"
)]
pub async fn test_database_connection(
    WorkspaceManagerWorkingCopy(workspace_manager): WorkspaceManagerWorkingCopy,
    Path(WorkspacePath { workspace_id: _ }): Path<WorkspacePath>,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    EffectiveWorkspaceRole(role): EffectiveWorkspaceRole,
    Json(request): Json<TestDatabaseConnectionRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let (tx, rx) = mpsc::channel::<ConnectionTestEvent>(100);
    let temp_db_name = format!("test_conn_{}", uuid::Uuid::new_v4());
    let repo_path = workspace_manager.config_manager.workspace_path();

    let database_config = match DatabaseConfigBuilder::build_configs(
        &WarehousesFormData {
            warehouses: vec![WarehouseConfig {
                r#type: request.warehouse.r#type.clone(),
                name: Some(temp_db_name.clone()),
                config: request.warehouse.config,
            }],
        },
        repo_path,
        user.id,
        &workspace_manager.secrets_manager,
    )
    .await
    {
        Ok(config) => config,
        Err(status) => {
            return Err(status);
        }
    };

    if database_config.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let user_id = user.id;
    let user_role = role.clone();
    tokio::spawn(async move {
        let start_time = std::time::Instant::now();

        // Progress: Start
        let _ = tx
            .send(ConnectionTestEvent::Progress {
                message: "Initiating connection test...".to_string(),
            })
            .await;

        // Set up scope guard to clean up secrets after testing
        let secret_name = format!("{}_PASSWORD", temp_db_name.to_uppercase());
        let secrets_manager = workspace_manager.secrets_manager.clone();
        let _cleanup_guard = guard((), move |_| {
            let secret_name = secret_name.clone();
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async move {
                    tracing::info!("Cleaning up temporary secret: {}", secret_name);
                    secrets_manager
                        .remove_secret(&secret_name)
                        .await
                        .unwrap_or_else(|e| {
                            tracing::error!(
                                "Failed to delete temporary secret {}: {}",
                                secret_name,
                                e
                            );
                        });
                })
            });
        });

        let db_config = &database_config[0];

        let _ = tx
            .send(ConnectionTestEvent::Progress {
                message: "Creating connector...".to_string(),
            })
            .await;

        // Airhouse types live outside `agentic-connector::ConnectorConfig`,
        // so the `database_to_connector_config` fast path below returns None
        // for them. Route through `OxyProjectContext::build_connector_for`
        // instead — it knows about both paths and surfaces real errors.
        if matches!(
            db_config.database_type,
            DatabaseType::Airhouse(_) | DatabaseType::AirhouseManaged(_)
        ) {
            let ctx = OxyProjectContext::new(workspace_manager.clone())
                .with_subject(user_id)
                .with_role(user_role.clone());
            let outcome = async {
                let connector = ctx
                    .build_connector_for(&db_config.name)
                    .await
                    .map_err(|e| e.to_string())?;
                connector
                    .execute_query("SELECT 1", 1)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok::<_, String>(())
            }
            .await;

            let elapsed = start_time.elapsed().as_millis() as u64;
            let event = match outcome {
                Ok(()) => ConnectionTestEvent::Complete {
                    result: TestDatabaseConnectionResponse {
                        success: true,
                        message: "Connection successful".to_string(),
                        connection_time_ms: Some(elapsed),
                        error_details: None,
                    },
                },
                Err(err) => ConnectionTestEvent::Complete {
                    result: TestDatabaseConnectionResponse {
                        success: false,
                        message: "Connection failed".to_string(),
                        connection_time_ms: Some(elapsed),
                        error_details: Some(err),
                    },
                },
            };
            let _ = tx.send(event).await;
            return;
        }

        // Fast path: drive the probe through `agentic-connector`.
        // For Snowflake browser auth we inject an SSO-URL callback so the
        // redirect URL is streamed back to the client via SSE before we block
        // waiting for the browser login to complete.
        // Only Snowflake private-key auth still falls through to the legacy path.
        if let Some(mut cfg) = database_to_connector_config(db_config, &workspace_manager).await {
            // Wire up the SSO URL channel for Snowflake browser auth.
            if let ConnectorConfig::Snowflake(ref mut sf_cfg) = cfg
                && let SnowflakeAuth::Browser {
                    ref mut sso_url_callback,
                    timeout_secs,
                    ..
                } = sf_cfg.auth
            {
                let (sso_tx, mut sso_rx) = mpsc::channel::<String>(1);
                let tx_sso = tx.clone();
                let timeout = timeout_secs;
                tokio::spawn(async move {
                    if let Some(sso_url) = sso_rx.recv().await {
                        let _ = tx_sso
                            .send(ConnectionTestEvent::BrowserAuthRequired {
                                sso_url,
                                message: "Please complete authentication in your browser"
                                    .to_string(),
                                timeout_secs: Some(timeout),
                            })
                            .await;
                    }
                });
                *sso_url_callback = Some(SsoUrlCallback(std::sync::Arc::new(move |url| {
                    let _ = sso_tx.try_send(url);
                })));
            }

            let _ = tx
                .send(ConnectionTestEvent::Progress {
                    message: "Testing connection...".to_string(),
                })
                .await;

            let outcome = async {
                let connector = agentic_connector::build_connector_async(cfg)
                    .await
                    .map_err(|e| e.to_string())?;
                connector
                    .execute_query("SELECT 1", 1)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok::<_, String>(())
            }
            .await;

            let elapsed = start_time.elapsed().as_millis() as u64;
            let event = match outcome {
                Ok(()) => ConnectionTestEvent::Complete {
                    result: TestDatabaseConnectionResponse {
                        success: true,
                        message: "Connection successful".to_string(),
                        connection_time_ms: Some(elapsed),
                        error_details: None,
                    },
                },
                Err(err) => ConnectionTestEvent::Complete {
                    result: TestDatabaseConnectionResponse {
                        success: false,
                        message: "Connection failed".to_string(),
                        connection_time_ms: Some(elapsed),
                        error_details: Some(err),
                    },
                },
            };
            let _ = tx.send(event).await;
            return;
        }

        // Fallback: legacy `Connector::from_db` — only Snowflake private-key
        // auth reaches here now; browser auth is handled via the fast path above.

        // Create SSO URL channel
        let (sso_tx, mut sso_rx) = mpsc::channel::<String>(1);

        let is_snowflake_browser = matches!(
            &db_config.database_type,
            DatabaseType::Snowflake(sf) if matches!(sf.auth_type, SnowflakeAuthType::BrowserAuth { .. })
        );

        // Spawn task to listen for SSO URL
        if is_snowflake_browser {
            let tx_clone = tx.clone();
            let db_config_clone = db_config.clone();
            tokio::spawn(async move {
                if let Some(sso_url) = sso_rx.recv().await {
                    let timeout =
                        if let DatabaseType::Snowflake(sf) = &db_config_clone.database_type {
                            if let SnowflakeAuthType::BrowserAuth {
                                browser_timeout_secs,
                                ..
                            } = &sf.auth_type
                            {
                                Some(*browser_timeout_secs)
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                    let _ = tx_clone
                        .send(ConnectionTestEvent::BrowserAuthRequired {
                            sso_url,
                            message: "Please complete authentication in your browser".to_string(),
                            timeout_secs: timeout,
                        })
                        .await;
                }
            });
        }

        // Create connector with SSO sender
        let connector = match Connector::from_db(
            db_config,
            &workspace_manager.config_manager,
            &workspace_manager.secrets_manager,
            None,
            None,
            None,
            if is_snowflake_browser {
                Some(sso_tx)
            } else {
                None
            },
            // Connection-test path; airhouse_managed flows through
            // build_connector_for above, not here, so subject /
            // workspace_id / effective_role are intentionally None —
            // `from_db` will refuse with a typed error if anyone reaches
            // this branch with airhouse_managed.
            None,
            None,
            None,
        )
        .await
        {
            Ok(conn) => conn,
            Err(e) => {
                tracing::error!("Failed to create connector: {}", e);
                let _ = tx
                    .send(ConnectionTestEvent::Complete {
                        result: TestDatabaseConnectionResponse {
                            success: false,
                            message: "Failed to create connector".to_string(),
                            connection_time_ms: None,
                            error_details: Some(e.to_string()),
                        },
                    })
                    .await;
                return;
            }
        };

        if is_snowflake_browser {
            let _ = tx
                .send(ConnectionTestEvent::Progress {
                    message: "Waiting for authentication...".to_string(),
                })
                .await;
        }

        let _ = tx
            .send(ConnectionTestEvent::Progress {
                message: "Testing connection...".to_string(),
            })
            .await;

        match connector.run_query("SELECT 1").await {
            Ok(_) => {
                let elapsed = start_time.elapsed().as_millis() as u64;
                let _ = tx
                    .send(ConnectionTestEvent::Complete {
                        result: TestDatabaseConnectionResponse {
                            success: true,
                            message: "Connection successful".to_string(),
                            connection_time_ms: Some(elapsed),
                            error_details: None,
                        },
                    })
                    .await;
            }
            Err(e) => {
                let elapsed = start_time.elapsed().as_millis() as u64;
                let _ = tx
                    .send(ConnectionTestEvent::Complete {
                        result: TestDatabaseConnectionResponse {
                            success: false,
                            message: "Connection failed".to_string(),
                            connection_time_ms: Some(elapsed),
                            error_details: Some(e.to_string()),
                        },
                    })
                    .await;
            }
        }
    });

    Ok(Sse::new(oxy::utils::create_sse_stream(rx)))
}

#[derive(Deserialize, ToSchema)]
pub struct InspectDatabaseQuery {
    /// Optional database name to inspect. When omitted, inspects all
    /// configured databases (rare during onboarding — usually one).
    pub database: Option<String>,
}

/// Lightweight schema/table discovery for the onboarding table picker.
///
/// Returns just `{ schema, table, column_count }` per table via a single
/// GROUP BY query per database (vs. one INFORMATION_SCHEMA.COLUMNS query per
/// schema in the full sync). Full column metadata is loaded later via
/// `POST /databases/sync?tables=...` once the user picks tables.
pub async fn inspect_database_handler(
    _: WorkspaceAdmin,
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Path(WorkspacePath {
        workspace_id: _workspace_id,
    }): Path<WorkspacePath>,
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
    Query(params): Query<InspectDatabaseQuery>,
) -> Result<impl IntoResponse, StatusCode> {
    let (tx, rx) = mpsc::channel::<InspectEvent>(100);
    let config = workspace_manager.config_manager.clone();
    let secrets_manager = workspace_manager.secrets_manager.clone();

    tokio::spawn(async move {
        let databases: Vec<oxy::config::model::Database> = match &params.database {
            Some(name) => match config.resolve_database(name) {
                Ok(db) => vec![db.clone()],
                Err(e) => {
                    let _ = tx
                        .send(InspectEvent::Error {
                            message: format!("Database '{name}' not found: {e}"),
                        })
                        .await;
                    return;
                }
            },
            None => config.list_databases().to_vec(),
        };

        if databases.is_empty() {
            let _ = tx
                .send(InspectEvent::Error {
                    message: "No databases configured".to_string(),
                })
                .await;
            return;
        }

        // Inspect each configured database and merge results into a single
        // schema list. In practice onboarding always passes a single
        // `database` param so this loop runs once.
        let mut merged_schemas = Vec::new();
        let mut total_tables: u32 = 0;
        let mut total_elapsed_ms: u64 = 0;
        for db in &databases {
            match inspect_database(db, &config, &secrets_manager, Some(tx.clone())).await {
                Ok(result) => {
                    total_tables += result.table_count;
                    total_elapsed_ms += result.elapsed_ms;
                    merged_schemas.extend(result.schemas);
                }
                Err(e) => {
                    tracing::error!("Schema inspection failed for {}: {}", db.name, e);
                    let _ = tx
                        .send(InspectEvent::Error {
                            message: format!("Inspection failed: {e}"),
                        })
                        .await;
                    return;
                }
            }
        }

        let _ = tx
            .send(InspectEvent::Complete {
                result: InspectionResult {
                    schema_count: merged_schemas.len() as u32,
                    table_count: total_tables,
                    schemas: merged_schemas,
                    elapsed_ms: total_elapsed_ms,
                },
            })
            .await;
    });

    Ok(Sse::new(oxy::utils::create_sse_stream(rx)))
}

/// Fast schema-only discovery: returns `{ schema, table_count }` per schema.
/// Preferred over `inspect_database_handler` for onboarding because it avoids
/// scanning every column in the warehouse.
pub async fn inspect_schemas_handler(
    _: WorkspaceAdmin,
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Path(WorkspacePath {
        workspace_id: _workspace_id,
    }): Path<WorkspacePath>,
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
    Query(params): Query<InspectDatabaseQuery>,
) -> Result<Json<SchemaListResult>, StatusCode> {
    let config = workspace_manager.config_manager;
    let secrets_manager = workspace_manager.secrets_manager;

    let database = match &params.database {
        Some(name) => config.resolve_database(name).map_err(|e| {
            tracing::error!("Database '{}' not found: {}", name, e);
            StatusCode::NOT_FOUND
        })?,
        None => {
            let dbs = config.list_databases();
            if dbs.len() != 1 {
                tracing::error!(
                    "inspect_schemas requires a `database` query param when {} databases are configured",
                    dbs.len()
                );
                return Err(StatusCode::BAD_REQUEST);
            }
            dbs[0].clone()
        }
    };

    match inspect_schemas(&database, &config, &secrets_manager).await {
        Ok(result) => Ok(Json(result)),
        Err(e) => {
            tracing::error!("Schema discovery failed for {}: {}", database.name, e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[derive(Deserialize, ToSchema)]
pub struct InspectSchemaTablesQuery {
    pub database: Option<String>,
    pub schema: String,
}

/// Lazy per-schema table listing: returns `{ name, column_count }` for each
/// table in the given schema. Called when the user expands a schema in the
/// onboarding picker.
pub async fn inspect_schema_tables_handler(
    _: WorkspaceAdmin,
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Path(WorkspacePath {
        workspace_id: _workspace_id,
    }): Path<WorkspacePath>,
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
    Query(params): Query<InspectSchemaTablesQuery>,
) -> Result<Json<SchemaTablesResult>, StatusCode> {
    let config = workspace_manager.config_manager;
    let secrets_manager = workspace_manager.secrets_manager;

    let database = match &params.database {
        Some(name) => config.resolve_database(name).map_err(|e| {
            tracing::error!("Database '{}' not found: {}", name, e);
            StatusCode::NOT_FOUND
        })?,
        None => {
            let dbs = config.list_databases();
            if dbs.len() != 1 {
                return Err(StatusCode::BAD_REQUEST);
            }
            dbs[0].clone()
        }
    };

    match inspect_schema_tables(&database, &params.schema, &config, &secrets_manager).await {
        Ok(result) => Ok(Json(result)),
        // The `schema` parameter is not a name the engine's grammar allows
        // (a BigQuery dataset ID): the caller's error, and no query was built.
        Err(oxy_shared::errors::OxyError::ArgumentError(reason)) => {
            tracing::warn!("Table discovery refused for {}: {}", database.name, reason);
            Err(StatusCode::BAD_REQUEST)
        }
        Err(e) => {
            tracing::error!(
                "Table discovery failed for {}.{}: {}",
                database.name,
                params.schema,
                e
            );
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[cfg(test)]
mod schema_tests {
    use super::*;

    #[test]
    fn database_schema_response_serializes() {
        let resp = DatabaseSchemaResponse {
            tables: vec![TableInfo {
                name: "users".to_string(),
                columns: vec![ColumnInfo {
                    name: "id".to_string(),
                    data_type: "int4".to_string(),
                }],
            }],
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"name\":\"users\""));
        assert!(json.contains("\"data_type\":\"int4\""));
    }

    #[test]
    fn empty_schema_serializes() {
        let resp = DatabaseSchemaResponse { tables: vec![] };
        let json = serde_json::to_string(&resp).unwrap();
        assert_eq!(json, r#"{"tables":[]}"#);
    }

    #[test]
    fn table_with_multiple_columns_serializes() {
        let resp = DatabaseSchemaResponse {
            tables: vec![TableInfo {
                name: "orders".to_string(),
                columns: vec![
                    ColumnInfo {
                        name: "id".to_string(),
                        data_type: "int8".to_string(),
                    },
                    ColumnInfo {
                        name: "total".to_string(),
                        data_type: "numeric".to_string(),
                    },
                ],
            }],
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"name\":\"orders\""));
        assert!(json.contains("\"data_type\":\"numeric\""));
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["tables"][0]["columns"].as_array().unwrap().len(), 2);
    }
}
