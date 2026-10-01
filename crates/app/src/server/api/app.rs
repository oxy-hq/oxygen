use std::collections::HashMap;
use std::path::PathBuf;

use crate::cli::commands::export_chart::export_charts_to_dir;
use crate::server::api::middlewares::role_guards::WorkspaceEditor;
use crate::server::api::middlewares::workspace_context::{
    EffectiveWorkspaceRole, PreaggCacheCtx, WorkspaceManagerReadOnly, WorkspaceManagerWorkingCopy,
};
use crate::server::service::app::{
    AppResultChartDisplay, AppResultData, AppResultDisplay, AppResultMarkdownDisplay,
    AppResultTableDisplay, AppService, DisplayWithError, GetAppResultResponse, TaskKind,
    TaskOutput, TaskResult, get_app_displays, render_control_default,
};
use axum::body::Body;
use axum::extract::{self, Path};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;

use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use entity::workspace_members::WorkspaceRole;
use oxy::config::WorkingCopy;
use oxy::config::model::{
    AppTaskMode, ControlConfig, DatabaseType, Display, DuckDBOptions, SQL, TaskType,
};
use oxy::exec_types::{Data, DataContainer};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tokio_util::io::ReaderStream;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Deserialize, Serialize, JsonSchema, ToSchema)]
pub struct AppItem {
    pub name: String,
    pub path: String,
    /// Human-friendly title pulled from the app's `title:` field, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Whether the app's `published` field is true. Unpublished apps are
    /// hidden from the left sidebar but remain visible in the IDE.
    #[serde(default)]
    pub published: bool,
    /// Whether the calling user is allowed to flip the publish state.
    /// True for any workspace role above Viewer.
    #[serde(default)]
    pub can_publish: bool,
}

#[derive(Deserialize, Default)]
pub struct ListAppsQuery {
    /// When true, only published apps are returned (used by the sidebar).
    /// Defaults to false so the IDE Objects view keeps seeing everything.
    #[serde(default)]
    pub published_only: bool,
    /// Active branch in the IDE. When set and not equal to the
    /// workspace's default branch, the compile-boundary path is
    /// bypassed (FS walk) so the user sees their working-copy edits.
    #[serde(default)]
    pub branch: Option<String>,
}

#[derive(Deserialize, Serialize)]
pub struct GetAppDataResponse {
    pub data: DataContainer,
    error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ApiErrorResponse {
    pub error: String,
}

/// Per-task information exposed to the frontend for client-side execution.
#[derive(Deserialize, Serialize)]
pub struct TaskClientInfo {
    /// Raw SQL template (may contain Jinja syntax like `{{ controls.x }}`).
    pub sql: String,
    /// Where to execute this task when controls change. `client` = DuckDB WASM (default),
    /// `server` = backend round-trip (needed for Snowflake, BigQuery, etc.).
    pub mode: AppTaskMode,
    /// Project-relative file paths that the SQL reads (e.g. `oxymart.csv`).
    /// The frontend downloads these once and registers them in DuckDB WASM so the
    /// original SQL runs unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_files: Vec<String>,
}

#[derive(Deserialize, Serialize)]
pub struct GetDisplaysResponse {
    pub displays: Vec<DisplayWithError>,
    pub controls: Vec<ControlConfig>,
    /// SQL templates and execution modes for each task, keyed by task name.
    /// Only `execute_sql` tasks with inline `sql_query` are included.
    pub tasks: HashMap<String, TaskClientInfo>,
}

fn decode_path(pathb64: &str) -> Result<PathBuf, StatusCode> {
    let decoded_bytes = BASE64_STANDARD.decode(pathb64).map_err(|e| {
        tracing::info!("Base64 decode error: {:?}", e);
        StatusCode::BAD_REQUEST
    })?;

    let path_string = String::from_utf8(decoded_bytes).map_err(|e| {
        tracing::info!("UTF8 conversion error: {:?}", e);
        StatusCode::BAD_REQUEST
    })?;

    Ok(PathBuf::from(path_string))
}

fn create_error_response(error_msg: String) -> GetAppDataResponse {
    GetAppDataResponse {
        data: DataContainer::None,
        error: Some(error_msg),
    }
}

/// List all apps in the project
///
/// Retrieves all app configurations available in the project. Returns app metadata
/// including names and relative paths. Apps are YAML-based configurations that define
/// data visualization and dashboard components.
#[utoipa::path(
    method(get),
    path = "/{workspace_id}/apps",
    params(
        ("workspace_id" = Uuid, Path, description = "Workspace UUID"),
        ("published_only" = Option<bool>, Query, description = "When true, return only apps with `published: true` (defaults to false)")
    ),
    responses(
        (status = OK, description = "Success", body = Vec<AppItem>, content_type = "application/json")
    ),
    security(
        ("ApiKey" = [])
    )
)]
pub async fn list_apps(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    role: Option<EffectiveWorkspaceRole>,
    extract::Query(query): extract::Query<ListAppsQuery>,
) -> Result<extract::Json<Vec<AppItem>>, StatusCode> {
    let config_manager = &workspace_manager.config_manager;

    // In local mode there's no role plumbing — treat the caller as an Owner so the
    // publish toggle is available. In cloud mode the workspace_middleware always
    // attaches an EffectiveWorkspaceRole.
    let can_publish = match role {
        Some(EffectiveWorkspaceRole(r)) => r > WorkspaceRole::Viewer,
        None => true,
    };

    // Compiled revision or working copy — the manager owns that choice, and the
    // middleware already pinned the revision this request reads. A retryable
    // error means "not compiled here yet", which is a 503 the frontend polls
    // through, not a 500 describing a fault on this node.
    let apps = config_manager
        .list_apps(query.published_only)
        .await
        .map_err(|e| {
            tracing::warn!(
                workspace_id = %workspace_manager.workspace_id,
                error = %e,
                "list_apps failed"
            );
            if e.retryable() {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        })?;

    Ok(extract::Json(
        apps.into_iter()
            .map(|entry| AppItem {
                name: entry.name,
                path: entry.file_path,
                title: entry.title,
                published: entry.published,
                can_publish,
            })
            .collect(),
    ))
}

/// Set `published: true` on the app's YAML file. Authoring permission
/// (Owner/Admin/Member) is required — Viewers are rejected with 403 by the
/// `WorkspaceEditor` extractor.
pub async fn publish_app(
    _: WorkspaceEditor,
    Path((_workspace_id, pathb64)): Path<(Uuid, String)>,
    WorkspaceManagerWorkingCopy(workspace_manager): WorkspaceManagerWorkingCopy,
) -> Result<extract::Json<AppItem>, StatusCode> {
    set_publish_state(workspace_manager, &pathb64, true).await
}

/// Set `published: false` on the app's YAML file.
pub async fn unpublish_app(
    _: WorkspaceEditor,
    Path((_workspace_id, pathb64)): Path<(Uuid, String)>,
    WorkspaceManagerWorkingCopy(workspace_manager): WorkspaceManagerWorkingCopy,
) -> Result<extract::Json<AppItem>, StatusCode> {
    set_publish_state(workspace_manager, &pathb64, false).await
}

async fn set_publish_state(
    workspace_manager: oxy::adapters::workspace::manager::WorkspaceManager<WorkingCopy>,
    pathb64: &str,
    published: bool,
) -> Result<extract::Json<AppItem>, StatusCode> {
    // super_read_only: a stateless serve replica must never write the working
    // copy. This route is IdeOnly (proxied to the ide), so this only fires if
    // that classification ever drifts — failing loud (421) beats silent loss.
    if !crate::server::role_manifest::process_is_fs_writable() {
        tracing::error!("refused app publish-state write on a stateless serve replica");
        return Err(StatusCode::MISDIRECTED_REQUEST);
    }
    let relative_path = decode_path(pathb64)?;
    let workspace_path = workspace_manager
        .config_manager
        .workspace_path()
        .to_path_buf();
    let workspace_path_canonical = workspace_path.canonicalize().map_err(|e| {
        tracing::error!(
            "Failed to canonicalize workspace path {:?}: {}",
            workspace_path,
            e
        );
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let absolute_path = workspace_path_canonical
        .join(&relative_path)
        .canonicalize()
        .map_err(|e| {
            tracing::warn!("App file not found: {:?} - {}", relative_path, e);
            StatusCode::NOT_FOUND
        })?;

    if !absolute_path.starts_with(&workspace_path_canonical) {
        tracing::warn!(
            "Rejected path traversal attempt outside workspace: {:?}",
            relative_path
        );
        return Err(StatusCode::FORBIDDEN);
    }

    if !absolute_path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(".app.yml"))
    {
        tracing::warn!(
            "Refused to set publish state on non-app file: {:?}",
            absolute_path
        );
        return Err(StatusCode::BAD_REQUEST);
    }

    let yaml = tokio::fs::read_to_string(&absolute_path)
        .await
        .map_err(|e| {
            tracing::warn!("Failed to read app file {:?}: {}", absolute_path, e);
            StatusCode::NOT_FOUND
        })?;

    let mut root: serde_yaml::Value = serde_yaml::from_str(&yaml).map_err(|e| {
        tracing::warn!("Failed to parse app YAML at {:?}: {}", absolute_path, e);
        StatusCode::UNPROCESSABLE_ENTITY
    })?;

    let mapping = root.as_mapping_mut().ok_or_else(|| {
        tracing::warn!("App YAML at {:?} is not a mapping", absolute_path);
        StatusCode::UNPROCESSABLE_ENTITY
    })?;
    mapping.insert(
        serde_yaml::Value::String("published".to_string()),
        serde_yaml::Value::Bool(published),
    );

    let serialized = serde_yaml::to_string(&root).map_err(|e| {
        tracing::error!("Failed to serialize app YAML: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    tokio::fs::write(&absolute_path, serialized)
        .await
        .map_err(|e| {
            tracing::error!("Failed to write app file {:?}: {}", absolute_path, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let name = relative_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string().replace(".app.yml", ""))
        .unwrap_or_default();
    let title = workspace_manager
        .config_manager
        .resolve_app(&relative_path)
        .await
        .ok()
        .and_then(|cfg| cfg.title.filter(|t| !t.trim().is_empty()));

    Ok(extract::Json(AppItem {
        name,
        path: relative_path.to_string_lossy().to_string(),
        title,
        published,
        can_publish: true,
    }))
}

/// Extract all single-quoted file paths (ending in .csv, .parquet, .json) from a SQL string.
/// These are project-relative source files the browser needs to download before running the
/// query in DuckDB WASM.
fn extract_sql_source_files(sql: &str) -> Vec<String> {
    let mut files = Vec::new();
    let chars: Vec<char> = sql.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\'' {
            let start = i + 1;
            let mut j = start;
            while j < chars.len() && chars[j] != '\'' {
                j += 1;
            }
            let content: String = chars[start..j].iter().collect();
            let lower = content.to_lowercase();
            if lower.ends_with(".csv") || lower.ends_with(".parquet") || lower.ends_with(".json") {
                files.push(content);
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    files.sort();
    files.dedup();
    files
}

pub async fn get_displays(
    Path((_workspace_id, pathb64)): Path<(Uuid, String)>,
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
) -> Result<extract::Json<GetDisplaysResponse>, StatusCode> {
    let path = decode_path(&pathb64)?;

    let (displays, controls, parsed_tasks) =
        match get_app_displays(workspace_manager.clone(), &path).await {
            Ok(result) => result,
            Err(e) => {
                tracing::debug!("Failed to get app displays: {:?}", e);
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
        };

    // Collect SQL templates for execute_sql tasks so the frontend can run them
    // client-side in DuckDB WASM without a server round-trip on control changes.
    let databases = workspace_manager.config_manager.list_databases();
    // The tasks come from the same parse that produced the displays above.
    // Re-reading them through `AppService` meant a second resolution down a
    // filesystem-bound path, for a list this function was already holding.
    let tasks: HashMap<String, TaskClientInfo> = parsed_tasks
        .into_iter()
        .filter_map(|task| {
            let sql_task = match &task.task_type {
                TaskType::ExecuteSQL(t) => t,
                _ => return None,
            };
            let sql = match &sql_task.sql {
                SQL::Query { sql_query } => sql_query.clone(),
                // sql_file tasks can't run in the browser without extra setup
                SQL::File { .. } => return None,
            };

            // The browser can only execute a task client-side when its database is a
            // local non-DuckLake DuckDB instance — that's the only backend the in-browser
            // DuckDB WASM runtime can reproduce (source files served as Parquet via
            // /apps/source/). Anything else (ClickHouse, Postgres, Snowflake, BigQuery,
            // DuckLake's PostgreSQL catalog + object-storage layout) is unreachable from
            // the browser, so we must mark those tasks server-mode regardless of what the
            // YAML declares — otherwise the frontend optimistically attempts a WASM run,
            // it fails, and a fallback `runApp` mutation flashes the centered loading
            // overlay until the server returns.
            let is_browser_runnable_database = databases.iter().any(|db| {
                db.name == sql_task.database
                    && matches!(
                        &db.database_type,
                        DatabaseType::DuckDB(d) if !matches!(&d.options, DuckDBOptions::DuckLake(_))
                    )
            });
            let source_files = extract_sql_source_files(&sql);

            // Tasks with source files (CSV/Parquet on disk) running against local DuckDB
            // can stream client-side — the browser downloads a Parquet version of each
            // source file and re-runs the SQL in DuckDB WASM. Everything else falls back
            // to the server.
            let effective_mode =
                if !is_browser_runnable_database || task.mode == AppTaskMode::Server {
                    AppTaskMode::Server
                } else {
                    task.mode.clone()
                };

            Some((
                task.name.clone(),
                TaskClientInfo {
                    sql,
                    mode: effective_mode,
                    source_files,
                },
            ))
        })
        .collect();

    // Render Jinja expressions in control defaults and options
    // (e.g. `default: "{{ now(fmt='%Y-%m-%d') }}"` or `options: ["{{ now(fmt='%Y') }}"]`)
    // so the frontend initialises widgets with computed values, not raw templates.
    let controls = controls
        .into_iter()
        .map(|mut c| {
            c.default = c.default.map(render_control_default);
            c.options = c
                .options
                .map(|opts| opts.into_iter().map(render_control_default).collect());
            c
        })
        .collect();

    Ok(extract::Json(GetDisplaysResponse {
        displays,
        controls,
        tasks,
    }))
}

pub async fn get_app_data(
    Path((_workspace_id, pathb64)): Path<(Uuid, String)>,
    WorkspaceManagerWorkingCopy(workspace_manager): WorkspaceManagerWorkingCopy,
    preagg_ctx: PreaggCacheCtx,
    extract::Query(_branch): extract::Query<BranchHintQuery>,
) -> Result<extract::Json<GetAppDataResponse>, StatusCode> {
    let path = decode_path(&pathb64)?;

    let mut app_service = AppService::new(workspace_manager.clone()).with_preagg(preagg_ctx);

    let app_tasks = match app_service.get_tasks(&path).await {
        Ok(tasks) => tasks,
        Err(e) => {
            tracing::debug!("Failed to get app tasks from path: {:?} {}", path, e);
            return Ok(extract::Json(create_error_response(format!(
                "Failed to get app tasks: {e}"
            ))));
        }
    };

    if let Some(cached_data) = app_service.try_load_cached_data(&path, &app_tasks).await {
        return Ok(extract::Json(GetAppDataResponse {
            data: cached_data,
            error: None,
        }));
    }

    let data = match app_service.run(&path, HashMap::new()).await {
        Ok(data) => data,
        Err(e) => {
            tracing::debug!("Failed to run app: {:?}", e);
            return Ok(extract::Json(create_error_response(format!(
                "Failed to run app: {e}"
            ))));
        }
    };

    Ok(extract::Json(GetAppDataResponse { data, error: None }))
}

/// `GET /{ws}/apps/{pathb64}/data-cached` — FleetOk. Returns the LAST cached app
/// data WITHOUT executing, so a stateless serve replica can show a dashboard's
/// last-known data when the ide is down. `try_load_cached_data` reads the local
/// cache, falling back to the S3 mirror (`runtime_artifact::app_data_key`), and
/// `get_tasks` resolves the app definition from the compile boundary. `404` when
/// nothing has been cached (the FE then keeps its "restarting" placeholder).
/// The ide-down degradation route: serve a cached answer when the singleton is
/// unreachable. It must therefore not need a working copy to do it — every read
/// on this path is the compile boundary, the local runtime state dir, or S3.
pub async fn get_app_data_cached(
    Path((_workspace_id, pathb64)): Path<(Uuid, String)>,
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    extract::Query(_branch): extract::Query<BranchHintQuery>,
) -> Result<extract::Json<GetAppDataResponse>, StatusCode> {
    let path = decode_path(&pathb64)?;
    let app_service = AppService::new(workspace_manager);
    let app_tasks = app_service.get_tasks(&path).await.map_err(|e| {
        // A genuine boundary/DB fault reads to the FE as "no cache" (it keeps the
        // placeholder either way), so log it here — otherwise a real failure on
        // this path is invisible in logs/metrics, indistinguishable from a miss.
        tracing::warn!(error = ?e, "get_app_data_cached: failed to resolve app tasks");
        StatusCode::NOT_FOUND
    })?;
    match app_service.try_load_cached_data(&path, &app_tasks).await {
        Some(data) => Ok(extract::Json(GetAppDataResponse { data, error: None })),
        None => Err(StatusCode::NOT_FOUND),
    }
}

pub async fn get_data(
    WorkspaceManagerWorkingCopy(workspace_manager): WorkspaceManagerWorkingCopy,
    Path((_workspace_id, pathb64)): Path<(Uuid, String)>,
) -> impl IntoResponse {
    let path_string = match decode_path(&pathb64) {
        Ok(path) => path.to_string_lossy().to_string(),
        Err(status) => return Err((status, "Invalid path".to_string())),
    };

    let state_path = workspace_manager
        .config_manager
        .resolve_state_dir()
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to resolve state dir: {e}"),
            )
        })?;
    let full_file_path = state_path
        .join(&path_string)
        .canonicalize()
        .map_err(|e| (StatusCode::NOT_FOUND, format!("File not found: {e}")))?;

    if !full_file_path.starts_with(&state_path) {
        return Err((StatusCode::FORBIDDEN, "Access denied".to_string()));
    }

    let file = match tokio::fs::File::open(&full_file_path).await {
        Ok(file) => file,
        Err(err) => return Err((StatusCode::NOT_FOUND, format!("File not found: {err}"))),
    };

    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    let mut headers = HeaderMap::new();
    headers.insert(
        "Cache-Control",
        HeaderValue::from_static("private, max-age=31536000, immutable"),
    );

    Ok((StatusCode::OK, headers, body))
}

/// Serve a project source file as Parquet so the browser can register it in DuckDB WASM
/// and re-run SQL client-side. The server reads the file via DuckDB (handling CSV, JSON,
/// Parquet, etc.) and re-serializes as Parquet — smaller and faster to parse than CSV.
///
/// Search order: (1) project root, (2) each local DuckDB database's file_search_path.
pub async fn get_source_file(
    WorkspaceManagerWorkingCopy(workspace_manager): WorkspaceManagerWorkingCopy,
    Path((_workspace_id, pathb64)): Path<(Uuid, String)>,
) -> impl IntoResponse {
    let path_string = match decode_path(&pathb64) {
        Ok(path) => path.to_string_lossy().to_string(),
        Err(status) => return Err((status, "Invalid path".to_string())),
    };

    let workspace_path = workspace_manager
        .config_manager
        .workspace_path()
        .to_path_buf();

    // Build candidate search directories.
    let mut search_dirs: Vec<PathBuf> = vec![workspace_path.clone()];
    for db in workspace_manager.config_manager.list_databases() {
        if let DatabaseType::DuckDB(duckdb) = &db.database_type
            && let DuckDBOptions::Local { file_search_path } = &duckdb.options
        {
            search_dirs.push(workspace_path.join(file_search_path));
        }
    }

    // Find the file under one of the search directories.
    // The canonicalized path is safe to use directly in SQL — the starts_with
    // check ensures it stays inside the project root.
    let full_path = search_dirs
        .iter()
        .find_map(|dir| {
            let candidate = dir.join(&path_string);
            candidate
                .canonicalize()
                .ok()
                .filter(|p| p.starts_with(&workspace_path))
        })
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("File not found: {path_string}"),
            )
        })?;

    // Use DuckDB to read the file and re-serialize as Parquet bytes.
    // This is done on a blocking thread because DuckDB is synchronous.
    let parquet_bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        use df_interchange::Interchange;
        use duckdb::Connection;
        use parquet::arrow::arrow_writer::ArrowWriter;

        let conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
        // Use the canonicalized absolute path so subdirectory references
        // (e.g. 'data/sales.csv') work correctly regardless of DuckDB's
        // default search path.
        let full_path_escaped = full_path.to_string_lossy().replace('\'', "''");

        let mut stmt = conn
            .prepare(&format!("SELECT * FROM '{full_path_escaped}'"))
            .map_err(|e| e.to_string())?;
        let arrow_stream = stmt.query_arrow([]).map_err(|e| e.to_string())?;
        let duckdb_batches: Vec<_> = arrow_stream.collect();

        // Empty CSVs / zero-row queries: skip the conversion that would
        // panic on an empty vec (see duckdb.rs) and emit an empty parquet.
        let batches = if duckdb_batches.is_empty() {
            Vec::new()
        } else {
            Interchange::from_arrow_58(duckdb_batches)
                .and_then(|ic| ic.to_arrow_58())
                .map_err(|e| e.to_string())?
        };
        let schema = batches
            .first()
            .map(|b| b.schema())
            .unwrap_or_else(|| std::sync::Arc::new(arrow::datatypes::Schema::empty()));

        let mut buf = Vec::new();
        let mut writer = ArrowWriter::try_new(&mut buf, schema, None).map_err(|e| e.to_string())?;
        for batch in batches {
            writer.write(&batch).map_err(|e| e.to_string())?;
        }
        writer.close().map_err(|e| e.to_string())?;
        Ok(buf)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let mut headers = HeaderMap::new();
    headers.insert(
        "Content-Type",
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(
        "Cache-Control",
        HeaderValue::from_static("private, max-age=3600"),
    );

    Ok((StatusCode::OK, headers, Body::from(parquet_bytes)))
}

#[derive(Deserialize, Default)]
pub struct RunAppBody {
    #[serde(default)]
    pub params: HashMap<String, JsonValue>,
}

pub async fn run_app(
    Path((_workspace_id, pathb64)): Path<(Uuid, String)>,
    WorkspaceManagerWorkingCopy(workspace_manager): WorkspaceManagerWorkingCopy,
    preagg_ctx: PreaggCacheCtx,
    extract::Query(_branch): extract::Query<BranchHintQuery>,
    body: Option<extract::Json<RunAppBody>>,
) -> Result<extract::Json<GetAppDataResponse>, StatusCode> {
    let path = decode_path(&pathb64)?;
    let params = body.map(|b| b.0.params).unwrap_or_default();

    let mut app_service = AppService::new(workspace_manager.clone()).with_preagg(preagg_ctx);
    let data = match app_service.run(&path, params).await {
        Ok(data) => data,
        Err(e) => {
            tracing::debug!("Failed to run app: {:?}", e);
            return Ok(extract::Json(create_error_response(format!(
                "Failed to run app: {e}"
            ))));
        }
    };

    Ok(extract::Json(GetAppDataResponse { data, error: None }))
}

/// Execute data app and get combined results (tasks + displays)
///
/// Executes a data app and returns both task execution results and display configurations.
/// This endpoint combines task outputs with their display representations, allowing consumers
/// to access both the raw data and its visual presentation.
#[derive(Deserialize, JsonSchema, ToSchema)]
pub struct AppResultQuery {
    /// When false (default), return cached result if available. When true, re-execute the app.
    #[serde(default)]
    pub refresh: bool,
    /// Active branch in the IDE. Plumbed through to the compile-boundary
    /// reader so feature-branch users see their working-copy edits
    /// instead of the promoted main definitions.
    #[serde(default)]
    pub branch: Option<String>,
}

/// Shared branch-only query for handlers that don't carry other query
/// params. Customer-facing (non-IDE) clients leave this unset; the
/// compile-boundary reader then has no branch hint and serves Postgres
/// directly when the workspace is promoted.
#[derive(Deserialize, Default)]
pub struct BranchHintQuery {
    #[serde(default)]
    pub branch: Option<String>,
}

fn get_result_cache_filename(app_path: &PathBuf) -> String {
    use xxhash_rust::xxh3::xxh3_64;
    // A staging pin (a workspace preview) gets its own cache file: the same app
    // rendered against a branch's model must never answer a live reader.
    let partition = crate::server::api::custom_apps_staging_pin::cache_partition();
    let key = format!("{}{partition}", app_path.to_string_lossy());
    let hash = xxh3_64(key.as_bytes());
    format!("{hash:x}.app.result.yml")
}

#[utoipa::path(
    method(post),
    path = "/{workspace_id}/apps/{pathb64}/result",
    params(
        ("workspace_id" = Uuid, Path, description = "Workspace UUID"),
        ("pathb64" = String, Path, description = "Base64-encoded path to data app file"),
        ("refresh" = Option<bool>, Query, description = "Re-execute app instead of returning cached result (defaults to false)")
    ),
    responses(
        (status = OK, description = "Execution completed successfully", body = GetAppResultResponse, content_type = "application/json"),
        (status = BAD_REQUEST, description = "Invalid request parameters"),
        (status = UNAUTHORIZED, description = "Invalid or missing API key"),
        (status = NOT_FOUND, description = "Data app not found"),
        (status = INTERNAL_SERVER_ERROR, description = "Execution failed or server error", body = ApiErrorResponse, content_type = "application/json")
    ),
    security(
        ("ApiKey" = [])
    )
)]
pub async fn get_app_result(
    Path((_workspace_id, pathb64)): Path<(Uuid, String)>,
    extract::Query(query): extract::Query<AppResultQuery>,
    WorkspaceManagerWorkingCopy(workspace_manager): WorkspaceManagerWorkingCopy,
    preagg_ctx: PreaggCacheCtx,
) -> (StatusCode, extract::Json<GetAppResultResponse>) {
    let path = match decode_path(&pathb64) {
        Ok(p) => p,
        Err(status) => {
            return (
                status,
                extract::Json(GetAppResultResponse {
                    success: false,
                    error_message: Some("Invalid base64 path encoding".to_string()),
                    result: None,
                }),
            );
        }
    };

    // Try to load cached result if not refreshing
    if !query.refresh
        && let Some(cached) = load_cached_result(&workspace_manager, &path).await
    {
        return (StatusCode::OK, extract::Json(cached));
    }

    // Execute the app to get task results. Branch comes from the
    // existing `AppResultQuery` so feature-branch users see their
    // working-copy edits when the Postgres-first path is on.
    let mut app_service = AppService::new(workspace_manager.clone()).with_preagg(preagg_ctx);

    // Get task names first (needed for response even if execution fails)
    let task_configs = match app_service.get_tasks(&path).await {
        Ok(tasks) => tasks,
        Err(e) => {
            tracing::debug!("Failed to get app tasks: {:?}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                extract::Json(GetAppResultResponse {
                    success: false,
                    error_message: Some(format!("Failed to get app tasks: {e}")),
                    result: None,
                }),
            );
        }
    };

    let execution_result = app_service.run(&path, HashMap::new()).await;

    let (tasks, execution_succeeded): (Vec<TaskResult>, bool) = match execution_result {
        Ok(DataContainer::Map(results)) => {
            // Convert the results map into TaskResult objects
            let tasks = task_configs
                .iter()
                .map(|task_config| {
                    let task_name = task_config.name.clone();
                    let task_type = TaskKind::from(task_config.kind());
                    let output = results.get(&task_name).and_then(data_container_to_output);

                    TaskResult {
                        task_name,
                        task_type,
                        output,
                        error: None,
                    }
                })
                .collect();
            (tasks, true)
        }
        Err(e) => {
            let error_msg = e.to_string();
            let tasks = task_configs
                .iter()
                .map(|task_config| TaskResult {
                    task_name: task_config.name.clone(),
                    task_type: TaskKind::from(task_config.kind()),
                    output: None,
                    error: Some(error_msg.clone()),
                })
                .collect();
            (tasks, false)
        }
        _ => {
            // Unexpected data container type
            let tasks = task_configs
                .iter()
                .map(|task_config| TaskResult {
                    task_name: task_config.name.clone(),
                    task_type: TaskKind::from(task_config.kind()),
                    output: None,
                    error: Some("Unexpected output format".to_string()),
                })
                .collect();
            (tasks, false)
        }
    };

    // Build a map of task_name -> output data for resolving display references
    let task_data_map: HashMap<String, TaskOutput> = tasks
        .iter()
        .filter_map(|t| t.output.as_ref().map(|o| (t.task_name.clone(), o.clone())))
        .collect();

    let typed_displays = match get_app_displays(workspace_manager.clone(), &path).await {
        Ok((displays, _controls, _tasks)) => displays,
        Err(e) => {
            tracing::debug!("Failed to get app displays: {:?}", e);
            vec![]
        }
    };

    // Check if there are any chart displays that need PNG export
    let has_charts = typed_displays.iter().any(|d| {
        matches!(
            d,
            DisplayWithError::Display(Display::LineChart(_))
                | DisplayWithError::Display(Display::BarChart(_))
                | DisplayWithError::Display(Display::PieChart(_))
        )
    });

    // Export charts to PNG if needed
    let mut chart_export_error: Option<String> = None;
    let chart_file_map: HashMap<i64, String> = if has_charts {
        // `charts_dir()`, the same resolver the reader 250 lines below uses.
        // `get_charts_dir()` is fallible — `resolve_state_dir` goes through
        // `require_root()` — and `unwrap_or_default()` turns that failure into
        // `PathBuf::new()`, so on a node whose root is not there yet the export
        // wrote PNGs relative to the process working directory and then
        // mirrored THOSE to S3. `export_charts_to_dir` creates the directory
        // itself, so nothing is lost by taking the infallible one.
        let charts_dir = workspace_manager.config_manager.charts_dir();
        let app_path_str = path.to_string_lossy().to_string();
        match export_charts_to_dir(&app_path_str, &charts_dir).await {
            Ok(map) => {
                // Mirror each exported PNG to S3 so a DIFFERENT serve replica
                // can serve it via GET /{ws}/apps/{path}/charts/{file} on the
                // round-robin fleet — otherwise the dashboard shows blank
                // charts. Best-effort; no-op without a bucket. See
                // server::runtime_artifact.
                for file_name in map.values() {
                    if let Ok(bytes) = tokio::fs::read(charts_dir.join(file_name)).await {
                        let key = crate::server::runtime_artifact::chart_key(
                            workspace_manager.workspace_id,
                            file_name,
                        );
                        crate::server::runtime_artifact::mirror(&key, bytes, "image/png").await;
                    }
                }
                map
            }
            Err(e) => {
                tracing::warn!("Failed to export charts: {:?}", e);
                chart_export_error = Some(e.to_string());
                HashMap::new()
            }
        }
    } else {
        HashMap::new()
    };

    // Build typed displays with resolved data
    let displays: Vec<AppResultDisplay> = typed_displays
        .into_iter()
        .enumerate()
        .filter_map(|(i, d)| match d {
            DisplayWithError::Display(display) => Some(match display {
                Display::LineChart(chart) => {
                    let file_path = chart_file_map.get(&(i as i64)).cloned();
                    AppResultDisplay::LineChart(AppResultChartDisplay {
                        file_name: file_path.clone(),
                        title: chart.title,
                        error: if file_path.is_none() {
                            chart_export_error.clone()
                        } else {
                            None
                        },
                    })
                }
                Display::BarChart(chart) => {
                    let file_path = chart_file_map.get(&(i as i64)).cloned();
                    AppResultDisplay::BarChart(AppResultChartDisplay {
                        file_name: file_path.clone(),
                        title: chart.title,
                        error: if file_path.is_none() {
                            chart_export_error.clone()
                        } else {
                            None
                        },
                    })
                }
                Display::PieChart(chart) => {
                    let file_path = chart_file_map.get(&(i as i64)).cloned();
                    AppResultDisplay::PieChart(AppResultChartDisplay {
                        file_name: file_path.clone(),
                        title: chart.title,
                        error: if file_path.is_none() {
                            chart_export_error.clone()
                        } else {
                            None
                        },
                    })
                }
                Display::Table(table) => {
                    let data = task_data_map
                        .get(&table.data)
                        .and_then(|o| serde_json::to_value(o).ok());
                    AppResultDisplay::Table(AppResultTableDisplay {
                        data,
                        title: table.title,
                    })
                }
                Display::Markdown(md) => AppResultDisplay::Markdown(AppResultMarkdownDisplay {
                    content: md.content,
                }),
                Display::Row(_) | Display::Controls(_) | Display::Control(_) => return None,
            }),
            DisplayWithError::Error(_) => None,
        })
        .collect();

    let response = GetAppResultResponse {
        success: execution_succeeded,
        error_message: None,
        result: Some(AppResultData { tasks, displays }),
    };

    // Only cache successful results to avoid permanently caching errors
    if execution_succeeded {
        save_cached_result(&workspace_manager, &path, &response).await;
    }

    (StatusCode::OK, extract::Json(response))
}

fn data_container_to_output(data: &DataContainer) -> Option<TaskOutput> {
    match data {
        DataContainer::Single(Data::Bool(b)) => Some(TaskOutput::Bool(*b)),
        DataContainer::Single(Data::Text(s)) => Some(TaskOutput::Text(s.clone())),
        DataContainer::Single(Data::Table(table_data)) => {
            // Always preserve the `{ file_path, json? }` shape — the FE
            // (`registerFromTableData` in
            // `web-app/src/components/AppPreview/Displays/utils.ts`)
            // reads `tableData.json` when present and falls back to a
            // network download of `tableData.file_path` when it's not.
            // Previously the `json.is_some()` branch parsed the JSON
            // string server-side and returned the bare records array,
            // dropping `file_path` — the FE then base64-encoded
            // `undefined` and hit the apps/file endpoint with a
            // missing path (404).
            serde_json::to_value(table_data).ok().map(TaskOutput::Table)
        }
        DataContainer::Single(Data::None) | DataContainer::None => None,
        DataContainer::List(items) => {
            let outputs: Vec<Box<TaskOutput>> = items
                .iter()
                .map(|item| Box::new(data_container_to_output(item).unwrap_or(TaskOutput::None)))
                .collect();
            if outputs.is_empty() {
                None
            } else {
                Some(TaskOutput::List(outputs))
            }
        }
        DataContainer::Map(map) => {
            let outputs: HashMap<String, Box<TaskOutput>> = map
                .iter()
                .filter_map(|(k, v)| data_container_to_output(v).map(|o| (k.clone(), Box::new(o))))
                .collect();
            if outputs.is_empty() {
                None
            } else {
                Some(TaskOutput::Map(outputs))
            }
        }
    }
}

async fn load_cached_result(
    workspace_manager: &oxy::adapters::workspace::manager::WorkspaceManager<WorkingCopy>,
    app_path: &PathBuf,
) -> Option<GetAppResultResponse> {
    let cache_name = get_result_cache_filename(app_path);
    let results_dir = workspace_manager
        .config_manager
        .get_app_results_dir()
        .await
        .ok()?;
    let cache_path = results_dir.join(cache_name);
    tokio::task::spawn_blocking(move || {
        if !cache_path.exists() {
            return None;
        }
        let file = std::fs::File::open(&cache_path).ok()?;
        let reader = std::io::BufReader::new(file);
        match serde_yaml::from_reader(reader) {
            Ok(data) => Some(data),
            Err(e) => {
                tracing::warn!("Failed to parse cached app result: {}", e);
                None
            }
        }
    })
    .await
    .ok()
    .flatten()
}

async fn save_cached_result(
    workspace_manager: &oxy::adapters::workspace::manager::WorkspaceManager<WorkingCopy>,
    app_path: &PathBuf,
    response: &GetAppResultResponse,
) {
    let cache_name = get_result_cache_filename(app_path);
    let Ok(results_dir) = workspace_manager.config_manager.get_app_results_dir().await else {
        return;
    };
    let cache_path = results_dir.join(cache_name);
    let response = response.clone();
    let _ = tokio::task::spawn_blocking(move || {
        let tmp_path = cache_path.with_extension("tmp");
        match std::fs::File::create(&tmp_path) {
            Ok(file) => {
                let writer = std::io::BufWriter::new(file);
                if let Err(e) = serde_yaml::to_writer(writer, &response) {
                    tracing::warn!("Failed to write cached app result: {}", e);
                    let _ = std::fs::remove_file(&tmp_path);
                    return;
                }
                if let Err(e) = std::fs::rename(&tmp_path, &cache_path) {
                    tracing::warn!("Failed to rename cache temp file: {}", e);
                    let _ = std::fs::remove_file(&tmp_path);
                }
            }
            Err(e) => {
                tracing::warn!("Failed to create cache temp file: {}", e);
            }
        }
    })
    .await;
}

/// Fetch chart image by file path
///
/// Retrieves a rendered chart image (PNG) by its file path. The file_path is returned
/// in chart display items (line_chart, bar_chart, pie_chart) from the result endpoint.
/// This endpoint serves the pre-rendered chart images for visualization.
#[utoipa::path(
    method(get),
    path = "/{workspace_id}/apps/{pathb64}/charts/{chart_path}",
    params(
        ("workspace_id" = Uuid, Path, description = "Workspace UUID"),
        ("pathb64" = String, Path, description = "Base64-encoded path to data app file"),
        ("chart_path" = String, Path, description = "File path of the chart image (from display item)")
    ),
    responses(
        (status = OK, description = "Image returned successfully", content_type = "image/png"),
        (status = UNAUTHORIZED, description = "Invalid or missing API key"),
        (status = NOT_FOUND, description = "Chart image not found"),
        (status = NOT_IMPLEMENTED, description = "Chart rendering not yet implemented")
    ),
    security(
        ("ApiKey" = [])
    )
)]
pub async fn get_chart_image(
    Path((workspace_id, pathb64, chart_path)): Path<(Uuid, String, String)>,
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
) -> Result<impl IntoResponse, StatusCode> {
    let _app_path = decode_path(&pathb64)?;

    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", HeaderValue::from_static("image/png"));
    headers.insert(
        "Cache-Control",
        HeaderValue::from_static("private, max-age=3600"),
    );

    // `charts_dir()`, not `get_charts_dir()`: same directory, but resolved
    // through `runtime_state_dir()`, whose working copy is an `Option`. A
    // replica has no working copy and still has a state dir, so the fast path
    // below simply misses and the S3 read-through under it answers.
    let charts_dir = workspace_manager.config_manager.charts_dir();

    // Fast path: the PNG is on THIS node's disk (it ran the export). Keep the
    // canonicalize + starts_with traversal guard.
    if let Ok(full_chart_path) = charts_dir.join(&chart_path).canonicalize() {
        if !full_chart_path.starts_with(&charts_dir) {
            return Err(StatusCode::FORBIDDEN);
        }
        if let Ok(file) = tokio::fs::File::open(&full_chart_path).await {
            let body = Body::from_stream(ReaderStream::new(file));
            return Ok((StatusCode::OK, headers, body));
        }
    }

    // Cross-node fallback: on the stateless serve fleet the export ran on a
    // DIFFERENT replica, so the local file is missing — read the S3 mirror.
    // Chart files are FLAT names; reject any path separator / traversal before
    // using `chart_path` as an object key.
    if chart_path.contains('/') || chart_path.contains("..") {
        return Err(StatusCode::BAD_REQUEST);
    }
    let key = crate::server::runtime_artifact::chart_key(workspace_id, &chart_path);
    if let Some(bytes) = crate::server::runtime_artifact::fetch(&key).await {
        return Ok((StatusCode::OK, headers, Body::from(bytes)));
    }

    tracing::debug!("Chart image not found (local + S3): {}", chart_path);
    Err(StatusCode::NOT_FOUND)
}

// ── App-builder run → save as app file ──────────────────────────────────────

#[derive(Serialize)]
pub struct SaveAppBuilderRunResponse {
    /// Base64-encoded app file path for use with AppPreview.
    pub app_path64: String,
    /// Project-relative path of the saved file.
    pub app_path: String,
}

/// Save a completed app-builder run's generated YAML as an `.app.yml` file
/// in the project directory and return the base64-encoded path for AppPreview.
///
/// The file is written to `generated/{run_id}.app.yml` within the project root.
pub async fn save_app_builder_run(
    Path((_workspace_id, run_id)): Path<(Uuid, String)>,
    WorkspaceManagerWorkingCopy(workspace_manager): WorkspaceManagerWorkingCopy,
) -> Result<extract::Json<SaveAppBuilderRunResponse>, StatusCode> {
    use agentic_runtime::entity::run as agentic_run;
    use sea_orm::EntityTrait;

    // super_read_only: a stateless serve replica must never write the working
    // copy (this writes `generated/{run_id}.app.yml`). IdeOnly route; fires only
    // on classification drift — 421 beats silently losing the generated app.
    if !crate::server::role_manifest::process_is_fs_writable() {
        tracing::error!("refused app builder-run save on a stateless serve replica");
        return Err(StatusCode::MISDIRECTED_REQUEST);
    }

    let db = oxy::database::client::establish_connection()
        .await
        .map_err(|e| {
            tracing::error!("db connect failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let run = agentic_run::Entity::find_by_id(&run_id)
        .one(&db)
        .await
        .map_err(|e| {
            tracing::error!("db query failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    let yaml = run.answer.ok_or(StatusCode::CONFLICT)?;

    // Write to {workspace_path}/generated/{run_id}.app.yml
    let workspace_path = workspace_manager.config_manager.workspace_path();
    let generated_dir = workspace_path.join("generated");
    tokio::fs::create_dir_all(&generated_dir)
        .await
        .map_err(|e| {
            tracing::error!("create generated dir: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let file_name = format!("{run_id}.app.yml");
    let full_path = generated_dir.join(&file_name);
    tokio::fs::write(&full_path, &yaml).await.map_err(|e| {
        tracing::error!("write app file: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let relative = format!("generated/{file_name}");
    let app_path64 = BASE64_STANDARD.encode(&relative);

    Ok(extract::Json(SaveAppBuilderRunResponse {
        app_path64,
        app_path: relative,
    }))
}

#[cfg(test)]
mod preview_cache_tests {
    use super::*;

    /// A data app's result cache is keyed by its path. Under a staging pin (a
    /// workspace preview) the same path gets a different file, so an answer
    /// rendered from a branch's model never serves a live reader — and the
    /// unpinned key is byte-for-byte what it always was.
    #[tokio::test]
    async fn a_preview_renders_into_its_own_result_cache() {
        let path = PathBuf::from("apps/sales.app.yml");
        let live = get_result_cache_filename(&path);
        let legacy = format!(
            "{:x}.app.result.yml",
            xxhash_rust::xxh3::xxh3_64(path.to_string_lossy().as_bytes())
        );
        assert_eq!(live, legacy, "unpinned keys do not move");

        let pinned = crate::server::api::custom_apps_staging_pin::with_staging_pin(
            Some(uuid::Uuid::new_v4()),
            async { get_result_cache_filename(&path) },
        )
        .await;
        assert_ne!(pinned, live);
        let other = crate::server::api::custom_apps_staging_pin::with_staging_pin(
            Some(uuid::Uuid::new_v4()),
            async { get_result_cache_filename(&path) },
        )
        .await;
        assert_ne!(pinned, other, "two previews do not share one either");
    }
}
