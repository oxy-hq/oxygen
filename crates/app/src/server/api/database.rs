use crate::server::api::middlewares::role_guards::WorkspaceAdmin;
use crate::server::api::middlewares::workspace_context::{
    EffectiveWorkspaceRole, WorkspaceManagerReadOnly, WorkspacePath,
};
use axum::{
    extract::{Json, Path, Query},
    http::StatusCode,
    response::{IntoResponse, sse::Sse},
};
use oxy::config::ResolveWorkspaceFile;
use oxy::config::model::SemanticModels;
use oxy::semantic::SemanticManager;
use oxy::semantic::inspector::{
    InspectEvent, InspectionResult, SchemaListResult, SchemaTablesResult, inspect_database,
    inspect_schema_tables, inspect_schemas,
};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use serde::{Deserialize, Serialize};
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
