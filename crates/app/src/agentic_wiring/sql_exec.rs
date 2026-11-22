//! How a Dev Portal query runs against one of the workspace's warehouses: the
//! connector the platform builds for it (the same builder, subject, role and
//! preview write hold every warehouse read uses), the ad-hoc row cap, and the
//! result shaped as JSON rows or a parquet file.
//!
//! The workspace runtime's half of the SQL surface. The IDE's SQL and semantic
//! execute handlers (`oxy-api-semantic`'s `data` and `semantic`) run on it, and so does
//! `oxy-api-world-model`'s `WorkspaceWarehouse` (through
//! `oxy_app::surface::world_model_exec`). It lived in `api::data` until
//! 2026-11-16 and moved down so those handlers can leave `oxy-app` without
//! taking a sibling surface's executor with them. The HTTP shaping of a failure
//! (`SqlErrorResponse`, `agentic_error_response`) stayed with the handlers until
//! they left too (Wave D2); it is [`super::sql_http`] now.
//!
//! Not a domain crate yet: it builds on `project_ctx::build_connector_for_db`
//! and writes results through `oxy_workspace_runtime::typed_stream` (the S3
//! result mirror), both of which go down with the base.

use std::sync::Arc;

use agentic_connector::{ConnectorError, DatabaseConnector, is_wrappable_select};
use entity::workspace_members::WorkspaceRole;
use oxy::adapters::{session_filters::SessionFilters, workspace::manager::WorkspaceManager};
use oxy::config::model::ConnectionOverrides;
use oxy::config::{ConfigManager, DiskSlot, ResolveWorkspaceFile};
use oxy_shared::errors::OxyError;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use oxy_workspace_runtime::typed_stream::{
    EMPTY_RESULT_SENTINEL, typed_stream_to_json_array, typed_stream_to_parquet,
};

#[derive(Serialize, Deserialize, Clone, Debug, ToSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum ResultFormat {
    Parquet,
    #[default]
    Json,
}

/// NAMED EXPLICITLY, because `oxy-api-custom-apps`' `data_plane::semantic_query` defines a DIFFERENT
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
    /// (`oxy-api-custom-apps`' `data_plane::query::run_sql_query`).
    #[serde(default)]
    pub untyped: bool,
}

/// Internal error type that preserves connector-level structure on the way
/// from `run_via_agentic_connector` to `sql_http::agentic_error_response`.
/// Public so the `semantic` handlers can compose compile → execute without
/// re-implementing the connector error mapping.
pub enum SqlExecuteError {
    Connector(ConnectorError),
    Other(OxyError),
}

impl From<OxyError> for SqlExecuteError {
    fn from(e: OxyError) -> Self {
        Self::Other(e)
    }
}

impl SqlExecuteError {
    pub fn debug_string(&self) -> String {
        match self {
            Self::Connector(e) => format!("{e:?}"),
            Self::Other(e) => format!("{e:?}"),
        }
    }
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
pub async fn run_via_agentic_connector<S: DiskSlot>(
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
    // column) and stream text cells. Mirrors `run_sql_query` in `oxy-api-custom-apps`' `data_plane::query`.
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
pub async fn run_with_connector(
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
            // The statement is the caller's (a Dev Portal query, or SQL built
            // from a tenant's model), so only its length is logged. The
            // warehouse's error text can still carry tenant values; this
            // module is in `DATA_PLANE_MODULE_PREFIXES`, so Sentry Logs keep
            // only the line's shape.
            tracing::warn!(error = %e, sql_len = sql.len(), "run_with_connector: query failed, returning empty result");
            vec![]
        }
    }
}

/// Build a connector for the given database name, scoped to `user_id`/`role`.
///
/// On either capability: what `OxyProjectContext::build_connector_for` does
/// with a subject and a role, without needing a `WorkingCopy` manager to
/// build the context from — same builder, same preview hold.
pub async fn build_connector<S: DiskSlot>(
    workspace_manager: &WorkspaceManager<S>,
    user_id: Uuid,
    role: WorkspaceRole,
    database: &str,
) -> Result<Arc<dyn DatabaseConnector>, OxyError>
where
    ConfigManager<S>: ResolveWorkspaceFile,
{
    crate::agentic_wiring::project_ctx::build_connector_for_db(
        workspace_manager,
        database,
        Some(user_id),
        Some(role),
    )
    .await
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
