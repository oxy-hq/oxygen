use crate::agentic_wiring::OxyProjectContext;
use crate::server::api::middlewares::workspace_context::{
    EffectiveWorkspaceRole, WorkspaceManagerReadOnly, WorkspaceManagerWorkingCopy, WorkspacePath,
};
use crate::server::api::typed_stream::{
    EMPTY_RESULT_SENTINEL, typed_stream_to_json_array, typed_stream_to_parquet,
};

// `ResultFormat` and `SemanticQueryResponse` previously lived in the
// retired `crate::server::api::semantic` module. They're inlined here
// because data.rs is now their only consumer.
#[derive(Serialize, Deserialize, Clone, Debug, ToSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum ResultFormat {
    Parquet,
    #[default]
    Json,
}

/// NAMED EXPLICITLY, because `projects::semantic_query` defines a DIFFERENT
/// type with the same Rust ident. utoipa derives a component name from the
/// ident alone, so without `as = ...` the two collapse into one
/// `SemanticQueryResponse` schema and whichever registers last wins — handing
/// a caller of `oxyc schema` the wrong body shape for the other endpoint, with
/// nothing anywhere saying so.
#[derive(Serialize, ToSchema)]
#[schema(as = SqlQueryResponse)]
#[serde(untagged)]
pub enum SemanticQueryResponse {
    Json(Vec<Vec<String>>),
    Parquet {
        file_name: String,
        is_preagg: bool,
        execution_time_ms: u64,
        /// `true` when the result hit the ad-hoc row cap (`IDE_MAX_ROWS`) and
        /// more rows exist in the warehouse. The IDE surfaces this so users
        /// know they're seeing a capped view, not the full result.
        #[serde(default)]
        truncated: bool,
    },
}
use crate::server::service::retrieval::{ReindexInput, reindex};
use agentic_connector::{
    ConnectorError, DatabaseConnector, QueryFailedDetails, is_wrappable_select,
};
use axum::extract::{self, Path};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use entity::workspace_members::WorkspaceRole;
use oxy::adapters::{session_filters::SessionFilters, workspace::manager::WorkspaceManager};
use oxy::config::model::ConnectionOverrides;
use oxy::config::{ConfigManager, DiskSlot, ResolveWorkspaceFile, WorkingCopy};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_shared::errors::OxyError;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Serialize, Deserialize, Clone, ToSchema)]
pub struct SQLParams {
    pub sql: String,
    pub database: String,

    #[serde(default)]
    pub filters: Option<SessionFilters>,

    #[serde(default)]
    #[schema(value_type = Object)]
    pub connections: Option<ConnectionOverrides>,

    #[serde(default)]
    pub result_format: Option<ResultFormat>,

    /// Skip airhouse's per-query `DESCRIBE` round-trip and return every column
    /// as text (the connector's `execute_query_full_untyped` path). The
    /// world-model dashboard sets this — its tiles already `::VARCHAR`-cast
    /// every column, so the DESCRIBE is pure overhead. Defaults to false (typed
    /// path). Mirrors the `untyped` flag on the public `/query` route
    /// (`projects::query::run_sql_query`).
    #[serde(default)]
    pub untyped: bool,
}

#[derive(Serialize, ToSchema)]
pub struct EmbeddingsBuildResponse {
    pub success: bool,
    pub message: String,
}

/// Structured error body returned by the SQL execute endpoints.
///
/// `message` is always populated. The remaining fields are surfaced when the
/// underlying connector returned a `ConnectorError::QueryFailed` whose driver
/// exposes vendor metadata (Postgres SQLSTATE / DETAIL / HINT / POSITION). The
/// IDE renders these as a structured block; older clients can keep displaying
/// `message` alone.
#[derive(Serialize, ToSchema)]
pub struct SqlErrorResponse {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<u32>,
    /// The SQL the connector reported as failing. May differ from the input
    /// (e.g. agentic temp-table wrapping); kept here so the IDE can highlight
    /// the right span when `position` is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sql: Option<String>,
}

/// Internal error type that preserves connector-level structure on the way
/// from `run_via_agentic_connector` to `agentic_error_response`. Exposed
/// crate-wide so the `semantic` module can compose compile → execute
/// without re-implementing the connector error mapping.
pub(crate) enum SqlExecuteError {
    Connector(ConnectorError),
    Other(OxyError),
}

impl From<OxyError> for SqlExecuteError {
    fn from(e: OxyError) -> Self {
        Self::Other(e)
    }
}

impl SqlExecuteError {
    pub(crate) fn debug_string(&self) -> String {
        match self {
            Self::Connector(e) => format!("{e:?}"),
            Self::Other(e) => format!("{e:?}"),
        }
    }
}

/// Shape an `SqlExecuteError` into a (status, body) pair. Structured fields
/// from `ConnectorError::QueryFailed(details)` propagate; everything else
/// degrades to a single-line `message`.
pub(crate) fn agentic_error_response(
    payload: &SQLParams,
    err: SqlExecuteError,
) -> (StatusCode, extract::Json<SqlErrorResponse>) {
    // Logged once the status is known. A 4xx is the query being wrong — bad
    // SQL, a missing column — which the user sees in the IDE and which breaks
    // nothing, so it is WARN. ERROR is kept for the 5xx classes, where the
    // warehouse or the driver failed. At ERROR every typo became a Sentry event
    // carrying the user's SQL.
    let error_debug = err.debug_string();

    // Status: 400 only for genuine user-side query errors (bad SQL,
    // missing columns, etc.). Upstream-unreachable (`ConnectionError`)
    // becomes 502 so the IDE shows a "warehouse is down" surface
    // distinguishable from "your SQL is wrong"; everything else
    // (decoder errors, internal driver bugs) is 500.
    let (status, body) = match err {
        SqlExecuteError::Connector(ConnectorError::QueryFailed(d)) => {
            let QueryFailedDetails {
                sql,
                message,
                code,
                detail,
                hint,
                position,
            } = d;
            (
                StatusCode::BAD_REQUEST,
                SqlErrorResponse {
                    message,
                    code,
                    detail,
                    hint,
                    position,
                    // Echo SQL only when it differs from what the user submitted —
                    // most of the time it's identical and would be noise.
                    sql: if sql != payload.sql { Some(sql) } else { None },
                },
            )
        }
        SqlExecuteError::Connector(ConnectorError::ConnectionError(msg)) => (
            StatusCode::BAD_GATEWAY,
            SqlErrorResponse {
                message: format!("connection error: {msg}"),
                code: None,
                detail: None,
                hint: None,
                position: None,
                sql: None,
            },
        ),
        SqlExecuteError::Connector(ConnectorError::Other(msg)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            SqlErrorResponse {
                message: msg,
                code: None,
                detail: None,
                hint: None,
                position: None,
                sql: None,
            },
        ),
        SqlExecuteError::Other(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            SqlErrorResponse {
                message: e.to_string(),
                code: None,
                detail: None,
                hint: None,
                position: None,
                sql: None,
            },
        ),
    };

    if status.is_server_error() {
        tracing::error!(
            database = %payload.database,
            sql = %truncate_sql_for_log(&payload.sql),
            error.debug = %error_debug,
            "SQL query execution failed"
        );
    } else {
        tracing::warn!(
            database = %payload.database,
            sql = %truncate_sql_for_log(&payload.sql),
            error.debug = %error_debug,
            "SQL query execution failed"
        );
    }

    (status, extract::Json(body))
}

/// Row cap applied to ad-hoc IDE Database/SQL-tab queries. Matches the
/// custom-app `/query` proxy `MAX_ROWS` so the two ad-hoc surfaces stay
/// consistent. 10k rows is plenty for eyeballing data in the IDE and keeps a
/// `SELECT col FROM huge_table` from pulling an entire column into the pod's
/// heap (the connector fully materializes the result before returning).
const IDE_MAX_ROWS: usize = 10_000;

/// Wrap a `SELECT`/`WITH` query in an outer `LIMIT` so an unbounded scan can't
/// OOM the pod, mirroring the custom-app `/query` proxy.
///
/// The gate is [`is_wrappable_select`], NOT `is_returning_statement`: only
/// statements valid as a derived table get wrapped. DDL/DML (`CREATE`/`INSERT`/
/// …) **and** introspection statements that return rows but are not legal
/// subqueries (`SHOW`/`DESCRIBE`/`PRAGMA`/`EXPLAIN`/`SUMMARIZE`/`CALL`/`PIVOT`/
/// `UNPIVOT`/`TABLE`/`FROM`-shorthand) pass through untouched. Wrapping those
/// would be a parse error — and DuckDB (the default local IDE backend) and
/// Postgres run them unwrapped today, so wrapping would regress them. They're
/// cheap; the connector byte guard is the OOM backstop for anything uncapped.
///
/// Returns the SQL to run plus whether a cap was applied. When capped, the
/// caller compares the returned row count to `IDE_MAX_ROWS` to decide if the
/// result was actually truncated (so the IDE can flag a partial view).
///
/// Note: trailing `;` is stripped before wrapping. A genuine multi-statement
/// string whose first keyword is `SELECT` (e.g. `SELECT 1; DROP TABLE x`) wraps
/// to invalid SQL and *fails* rather than executing — an acceptable (safe)
/// outcome for what is already unsupported on the single-statement full path.
fn cap_ide_result_rows(sql: &str) -> (String, bool) {
    if is_wrappable_select(sql) {
        let trimmed = sql.trim_end().trim_end_matches(';').trim_end();
        let wrapped =
            format!("SELECT * FROM (\n{trimmed}\n) AS oxy_ide_query LIMIT {IDE_MAX_ROWS}");
        (wrapped, true)
    } else {
        (sql.to_string(), false)
    }
}

/// Execute a SQL query through `agentic-connector` and shape the response
/// according to the requested `result_format`. Every `DatabaseType` in
/// `oxy::config::model` has a landing spot in `OxyProjectContext`, so this
/// is now the single path for every Dev Portal query.
pub(crate) async fn run_via_agentic_connector<S: DiskSlot>(
    workspace_manager: &WorkspaceManager<S>,
    user_id: Uuid,
    role: WorkspaceRole,
    payload: &SQLParams,
) -> Result<SemanticQueryResponse, SqlExecuteError>
where
    ConfigManager<S>: ResolveWorkspaceFile,
{
    let connector = crate::agentic_wiring::project_ctx::build_connector_for_db(
        workspace_manager,
        &payload.database,
        Some(user_id),
        Some(role),
    )
    .await?;

    // The 10k cap applies to *every* caller of this shared function — not just
    // the IDE Database tab, but also `/semantic`, `world_model_graph`, and the
    // metric-tree runner. Those internal callers already build their own,
    // smaller `LIMIT`, so the outer wrap is a harmless ceiling. Caveat: the
    // `ResultFormat::Json` branch below returns a bare `Vec<Vec<String>>`
    // (`SemanticQueryResponse::Json`) with no field for a `truncated` flag, so
    // neither a row-cap fill nor a connector byte-truncation (which can now stop
    // *below* 10k on wide rows) is signalled on this branch. Acceptable: the IDE
    // — the only surface that shows truncation to a human — always requests
    // Parquet (which does carry the flag). The external custom-app `/query` +
    // `/semantic-query` proxies use `typed_stream_to_json_objects`, which DOES
    // return the connector flag, so programmatic JSON consumers there are covered.
    let (sql_to_run, capped) = cap_ide_result_rows(&payload.sql);

    let query_start = std::time::Instant::now();
    // The world-model dashboard opts into the untyped fast path: skip airhouse's
    // per-query `DESCRIBE` round-trip (its tiles already `::VARCHAR`-cast every
    // column) and stream text cells. Mirrors `run_sql_query` in projects::query.
    // Every other caller leaves `untyped` false and keeps the typed path.
    let stream = if payload.untyped {
        connector.execute_query_full_untyped(&sql_to_run).await
    } else {
        connector.execute_query_full(&sql_to_run).await
    }
    .map_err(SqlExecuteError::Connector)?;
    let execution_time_ms = query_start.elapsed().as_millis() as u64;

    let result_format = payload
        .result_format
        .as_ref()
        .unwrap_or(&ResultFormat::Json);
    match result_format {
        ResultFormat::Parquet => {
            let (file_name, row_count, connector_truncated) =
                typed_stream_to_parquet(stream, workspace_manager)
                    .await
                    .map_err(SqlExecuteError::Other)?;
            if file_name == EMPTY_RESULT_SENTINEL {
                // DDL/DML or zero-column result — return empty JSON so the
                // frontend shows an empty table instead of a broken Parquet read.
                Ok(SemanticQueryResponse::Json(vec![]))
            } else {
                Ok(SemanticQueryResponse::Parquet {
                    file_name,
                    is_preagg: false,
                    execution_time_ms,
                    // Truncated if the soft row cap filled OR the connector hit
                    // its byte/row backstop. The latter catches wide-row
                    // byte-truncation: the row count can stay under the soft cap
                    // yet the result is still partial. (An exactly-`IDE_MAX_ROWS`
                    // natural result reads as truncated too, which is acceptable.)
                    truncated: (capped && row_count >= IDE_MAX_ROWS) || connector_truncated,
                })
            }
        }
        ResultFormat::Json => {
            let data = typed_stream_to_json_array(stream)
                .await
                .map_err(SqlExecuteError::Other)?;
            Ok(SemanticQueryResponse::Json(data))
        }
    }
}

/// Execute SQL against an already-built connector, returning JSON rows.
/// Use this when running multiple queries against the same database in one
/// handler — build the connector once with `OxyProjectContext::build_connector_for`
/// and pass it here to avoid paying the initialization cost per query.
/// Execute SQL against an already-built connector, returning data rows (header
/// row 0 stripped). The header is consistent with what `run_via_agentic_connector`
/// returns — callers that previously did `.skip(1)` after `run_via_agentic_connector`
/// can use this directly with `.first()` / `.next()`.
pub(crate) async fn run_with_connector(
    connector: &Arc<dyn DatabaseConnector>,
    sql: &str,
) -> Vec<Vec<String>> {
    match connector.execute_query_full(sql).await {
        Ok(stream) => typed_stream_to_json_array(stream)
            .await
            .unwrap_or_default()
            .into_iter()
            .skip(1)
            .collect(),
        Err(e) => {
            tracing::warn!(error = %e, sql, "run_with_connector: query failed, returning empty result");
            vec![]
        }
    }
}

/// Build a connector for the given database name, scoped to `user_id`/`role`.
pub(crate) async fn build_connector(
    workspace_manager: &WorkspaceManager<WorkingCopy>,
    user_id: Uuid,
    role: WorkspaceRole,
    database: &str,
) -> Result<Arc<dyn DatabaseConnector>, OxyError> {
    let ctx = OxyProjectContext::new(workspace_manager.clone())
        .with_subject(user_id)
        .with_role(role);
    ctx.build_connector_for(database).await
}

/// Cap SQL length in structured log fields so one bad query doesn't flood the
/// log pipeline. The error response keeps the database name intact; the SQL
/// preview is just for operator triage.
fn truncate_sql_for_log(sql: &str) -> String {
    const MAX: usize = 500;
    if sql.len() <= MAX {
        sql.to_string()
    } else {
        // Find the largest char boundary at or below MAX so we don't split a
        // multi-byte UTF-8 sequence.
        let boundary = (0..=MAX)
            .rev()
            .find(|i| sql.is_char_boundary(*i))
            .unwrap_or(0);
        format!(
            "{}… [truncated, {} bytes total]",
            &sql[..boundary],
            sql.len()
        )
    }
}

pub async fn execute_sql(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    EffectiveWorkspaceRole(role): EffectiveWorkspaceRole,
    Path(WorkspacePath {
        workspace_id: _workspace_id,
    }): Path<WorkspacePath>,
    extract::Json(payload): extract::Json<SQLParams>,
) -> Result<extract::Json<SemanticQueryResponse>, (StatusCode, extract::Json<SqlErrorResponse>)> {
    run_via_agentic_connector(&workspace_manager, user.id, role, &payload)
        .await
        .map(extract::Json)
        .map_err(|e| agentic_error_response(&payload, e))
}

/// Run SQL against one of the workspace's configured databases.
///
/// The workhorse of the API for anything data-shaped. `database` names a
/// connection from the workspace's `config.yml`; `GET /{workspace_id}/databases`
/// lists them. Mounted on BOTH the user-token surface
/// (`/api/{workspace_id}/sql/query`) and the API-key surface
/// (`/external/api/{workspace_id}/sql/query`) — same handler, same body.
///
/// **The response shape depends on `result_format`, and the default is not an
/// object.** With `json` (the default) the body is an array of arrays of
/// strings, **header row first**: `[["id","name"],["1","ada"]]`. With `parquet`
/// it is an object naming a file, and only that variant carries `truncated`.
/// The two are an untagged enum, so a client must not assume a field is there.
#[utoipa::path(
    method(post),
    path = "/{workspace_id}/sql/query",
    params(
        ("workspace_id" = Uuid, Path, description = "Workspace UUID")
    ),
    request_body = SQLParams,
    responses(
        (status = OK, description = "`json` (default): rows as arrays of strings, header row first. `parquet`: an object naming the written file.", body = SemanticQueryResponse, content_type = "application/json"),
        (status = BAD_REQUEST, description = "The query failed; `message` always set, vendor fields when the driver exposes them", body = SqlErrorResponse, content_type = "application/json")
    ),
    security(
        ("ApiKey" = []),
        ("BearerAuth" = [])
    )
)]
pub async fn execute_sql_query(
    workspace: WorkspaceManagerReadOnly,
    user: AuthenticatedUserExtractor,
    role: EffectiveWorkspaceRole,
    path: Path<WorkspacePath>,
    payload: extract::Json<SQLParams>,
) -> Result<extract::Json<SemanticQueryResponse>, (StatusCode, extract::Json<SqlErrorResponse>)> {
    execute_sql(workspace, user, role, path, payload).await
}

// TODO: may want to rename this and the `reindex()` function below as we're doing more
//       only conditionally reindexing and doing more than just building embeddings:
//         - constructing retrieval items to store in lancedb
//         - calculating inclusion radius for each retrieval item
//         - caching enum values for each variable so they can be detected at query time
pub async fn build_embeddings(
    WorkspaceManagerWorkingCopy(workspace_manager): WorkspaceManagerWorkingCopy,
    Path(WorkspacePath {
        workspace_id: _workspace_id,
    }): Path<WorkspacePath>,
) -> Result<extract::Json<EmbeddingsBuildResponse>, Response> {
    handle_omni_sync(&workspace_manager)
        .await
        .map_err(|e| StatusCode::from(e).into_response())?;
    let config_manager = workspace_manager.config_manager;
    let secret_manager = workspace_manager.secrets_manager;
    let drop_all_tables = false;

    match reindex(ReindexInput {
        config: config_manager,
        secrets_manager: secret_manager,
        drop_all_tables,
    })
    .await
    {
        Ok(_) => Ok(extract::Json(EmbeddingsBuildResponse {
            success: true,
            message: "Embeddings built successfully".to_string(),
        })),
        Err(e) => {
            tracing::error!("Embeddings build failed: {}", e);
            Ok(extract::Json(EmbeddingsBuildResponse {
                success: false,
                message: format!("Embeddings build failed: {e}"),
            }))
        }
    }
}

async fn handle_omni_sync(workspace: &WorkspaceManager<WorkingCopy>) -> Result<(), OxyError> {
    use crate::server::service::omni_sync::OmniSyncService;
    use omni::{OmniApiClient, OmniError as AdapterOmniError};

    let workspace_path = workspace.config_manager.workspace_path();

    let config = workspace.config_manager.clone();

    // Get all Omni integration configurations - if none found, skip silently
    let omni_integrations: Vec<_> = config
        .get_config()
        .integrations
        .iter()
        .filter_map(|integration| match &integration.integration_type {
            oxy::config::model::IntegrationType::Omni(omni_integration) => {
                Some((integration.name.clone(), omni_integration.clone()))
            }
            _ => None,
        })
        .collect();

    if omni_integrations.is_empty() {
        // No Omni integrations configured, skip silently
        return Ok(());
    }

    tracing::info!(
        "Synchronizing {} Omni integration(s)",
        omni_integrations.len()
    );

    let mut all_sync_results = Vec::new();
    let mut total_successful_topics = Vec::new();

    for (integration_name, omni_integration) in omni_integrations {
        tracing::info!(integration = %integration_name, "Processing Omni integration");

        // Resolve API key from environment variable
        let api_key = workspace
            .secrets_manager
            .resolve_secret(&omni_integration.api_key_var)
            .await?
            .unwrap();
        let base_url = omni_integration.base_url.clone();
        let topics = omni_integration.topics.clone();

        tracing::debug!(integration = %integration_name, topic_count = topics.len(), "Synchronizing Omni metadata");
        let topics_to_sync: Vec<_> = topics.iter().collect();

        let api_client =
            OmniApiClient::new(base_url.clone(), api_key.clone()).map_err(|e| match e {
                AdapterOmniError::ConfigError(msg) => {
                    OxyError::ConfigurationError(format!("Omni configuration error: {}", msg))
                }
                _ => OxyError::RuntimeError(format!("Failed to create Omni API client: {}", e)),
            })?;

        let sync_service =
            OmniSyncService::new(api_client, workspace_path, integration_name.clone());

        tracing::debug!("Fetching metadata from Omni API");

        let mut integration_results = Vec::new();
        for topic in &topics_to_sync {
            tracing::debug!(topic = %topic.name, model = %topic.model_id, "Syncing Omni topic");
            let sync_result = sync_service
                .sync_metadata(&topic.model_id, &topic.name)
                .await
                .map_err(|e| {
                    OxyError::RuntimeError(format!(
                        "Sync operation failed for topic '{}' (model '{}'): {}",
                        topic.name, topic.model_id, e
                    ))
                })?;
            integration_results.push(sync_result);
        }

        if let Some(first_result) = integration_results.into_iter().next() {
            total_successful_topics.extend(first_result.successful_topics.clone());
            all_sync_results.push(first_result);
        }
    }

    tracing::info!("Omni synchronization completed");

    if !all_sync_results.is_empty() {
        let overall_success = all_sync_results.iter().all(|r| r.is_success());
        let partial_success = all_sync_results.iter().any(|r| r.is_partial_success());

        if overall_success {
            tracing::info!("All integrations synchronized successfully");
        } else if partial_success {
            tracing::warn!("Partial synchronization completed with some errors");
            for sync_result in &all_sync_results {
                if let Some(error_summary) = sync_result.error_summary() {
                    tracing::warn!(error = %error_summary, "Omni sync errors encountered");
                }
            }
        } else {
            tracing::error!("Some integrations failed to synchronize");
            for sync_result in &all_sync_results {
                if let Some(error_summary) = sync_result.error_summary() {
                    tracing::error!(error = %error_summary, "Omni sync errors encountered");
                }
            }
            return Err(OxyError::RuntimeError(
                "Some Omni sync operations failed".to_string(),
            ));
        }

        if !total_successful_topics.is_empty() {
            tracing::info!(topics = ?total_successful_topics, "Successfully synchronized topics");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_returning_selects() {
        let (out, capped) = cap_ide_result_rows("SELECT guid FROM order_selections");
        assert!(capped);
        assert!(out.contains("AS oxy_ide_query LIMIT 10000"), "{out}");
        assert!(out.contains("SELECT guid FROM order_selections"), "{out}");
    }

    #[test]
    fn caps_with_cte() {
        let (out, capped) = cap_ide_result_rows("WITH t AS (SELECT 1) SELECT * FROM t");
        assert!(capped);
        assert!(out.contains("LIMIT 10000"), "{out}");
    }

    #[test]
    fn strips_trailing_semicolon_before_wrapping() {
        let (out, capped) = cap_ide_result_rows("SELECT 1;");
        assert!(capped);
        // The inner statement must not retain the `;`, which would make
        // `SELECT * FROM (SELECT 1;) ...` a syntax error.
        assert!(
            out.contains("SELECT 1\n) AS oxy_ide_query LIMIT 10000"),
            "{out}"
        );
    }

    #[test]
    fn passes_ddl_through_untouched() {
        // DDL/DML must NOT be wrapped — wrapping turns them into syntax errors,
        // and the IDE legitimately runs these. They are also not "capped", so no
        // truncation is ever reported for them.
        for sql in [
            "CREATE TABLE t (id Int64)",
            "INSERT INTO t VALUES (1)",
            "DROP TABLE t",
            "ALTER TABLE t ADD COLUMN c Int64",
            "SET max_threads = 4",
        ] {
            let (out, capped) = cap_ide_result_rows(sql);
            assert_eq!(out, sql, "DDL/DML should pass through");
            assert!(!capped, "DDL/DML must not be marked capped: {sql}");
        }
    }

    #[test]
    fn ignores_leading_comments_when_classifying() {
        // A leading comment must not hide the SELECT keyword from the gate.
        let (out, capped) = cap_ide_result_rows("-- pick guids\nSELECT guid FROM t");
        assert!(capped);
        assert!(out.contains("LIMIT 10000"), "{out}");
    }

    #[test]
    fn passes_introspection_through_unwrapped() {
        // These return rows but are NOT valid derived tables — wrapping them in
        // `SELECT * FROM (...)` is a parse error in every dialect. DuckDB (the
        // default local IDE backend) and Postgres run them directly today, so
        // they MUST pass through unwrapped. The connector byte guard is the OOM
        // backstop. Regression guard for the over-broad `is_returning_statement`
        // gate that this PR replaced with `is_wrappable_select`.
        for sql in [
            "SHOW TABLES",
            "DESCRIBE my_table",
            "DESC my_table",
            "PRAGMA database_list",
            "EXPLAIN SELECT 1",
            "SUMMARIZE my_table",
            "CALL pragma_version()",
            "PIVOT t ON x USING sum(y)",
            "UNPIVOT t ON a, b",
            "TABLE my_table",
            "FROM my_table",
            "VALUES (1), (2)",
        ] {
            let (out, capped) = cap_ide_result_rows(sql);
            assert_eq!(out, sql, "introspection must pass through unwrapped: {sql}");
            assert!(!capped, "introspection must not be marked capped: {sql}");
        }
    }
}
