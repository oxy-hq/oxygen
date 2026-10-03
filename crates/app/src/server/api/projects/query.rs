//! `POST /api/projects/{project_id}/query` — SQL proxy for custom-app bundles.
//!
//! Accepts a JSON body `{ sql, database?, params? }`. Cookie auth gates the
//! request; the caller must be a member of the org that owns the
//! workspace identified by `project_id`.
//!
//! `params` fill the `{{ params.X | sqlquote }}` / `{{ params.X }}`
//! placeholders in `sql`, quoted by the rule of `database`'s engine — see
//! [`super::query_params`]. Everything below applies to the SQL that results.
//!
//! Security gates applied before execution:
//!   1. Origin check — `Origin` / `Referer` must match the request's own host
//!      (same-domain deployment) or be one of the canonical Vite dev origins,
//!      or be absent for non-browser clients.
//!   2. SELECT/WITH-only — the first non-comment, non-whitespace token must be
//!      SELECT or WITH. Rejects DELETE / DROP / INSERT / UPDATE at the proxy.
//!   3. 10 000-row cap — the query is wrapped in an outer `LIMIT` subquery so
//!      a `SELECT * FROM huge_table` cannot OOM the server. If the caller
//!      already supplies a smaller LIMIT, the outer wrap is a no-op ceiling.
//!
//! Returns `{ columns, rows, truncated }` on success — `truncated` is `true`
//! when the result was capped at `MAX_ROWS`.

use std::sync::Arc;

use agentic_connector::{ConnectorError, DatabaseConnector};
use axum::Json;
use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use indexmap::IndexMap;
use oxy_shared::errors::OxyError;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tracing::{error, instrument, warn};
use uuid::Uuid;

use crate::agentic_wiring::string_literal::string_literal_of;
use crate::server::api::custom_apps_gates::{check_custom_app_gates, parse_versioned_body};
use crate::server::api::typed_stream::typed_stream_to_json_objects;
use utoipa::ToSchema;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum number of rows returned by a single query. The SQL is wrapped in an
/// outer subquery with this limit so the cap is applied at the warehouse, not
/// after materialising the full result set in memory.
const MAX_ROWS: usize = 10_000;

// ── Error helper ─────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct ApiErr {
    message: String,
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (
        status,
        Json(ApiErr {
            message: msg.into(),
        }),
    )
        .into_response()
}

// ── Request / Response shapes ────────────────────────────────────────────────

/// Request body for `POST /api/projects/{project_id}/query`.
///
/// `sql` is required (non-empty). `database` is optional.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryRequest {
    /// Raw SQL to execute. Required and non-empty.
    pub sql: String,
    /// Database name from config.yml. Falls back to the project's default
    /// database when absent or empty.
    pub database: Option<String>,
    /// Skip native column-type introspection and return every value as text
    /// (the connector's `execute_query_full_untyped` path). The world-model
    /// dashboard sets this: its tiles already `::VARCHAR`-cast every column, so
    /// airhouse's per-query `DESCRIBE` round-trip is pure overhead. Defaults to
    /// false (typed path) for every other caller.
    #[serde(default)]
    pub untyped: bool,
    /// Values for the `{{ params.X | sqlquote }}` / `{{ params.X }}`
    /// placeholders in `sql`: strings, numbers, booleans or null. Absent,
    /// `sql` is run as sent and nothing in it is read as a placeholder.
    #[serde(default)]
    #[schema(value_type = Option<Object>)]
    pub params: Option<serde_json::Map<String, JsonValue>>,
}

/// Response body — columnar table shape.
///
/// Column order matches first-seen key order across all rows (using an
/// `IndexMap` for stable iteration). Missing cells in a row become
/// `JsonValue::Null`. `truncated` is `true` when the result hit `MAX_ROWS`.
#[derive(Debug, Serialize, ToSchema)]
pub struct QueryResponse {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<JsonValue>>,
    pub truncated: bool,
}

// ── Handler ───────────────────────────────────────────────────────────────────

/// `POST /api/projects/{project_id}/query`
///
/// Auth → origin check → body validation → workspace lookup → org-membership →
/// connector resolution → SELECT gate → SQL execution → columnar reshape.
/// Run SQL through a project, against its default (or a named) database.
///
/// The project-scoped twin of `/{workspace_id}/sql/query`, and the one a
/// custom app uses: the project id carries the workspace, so no workspace id is
/// needed. `deny_unknown_fields` is on, so a misspelled key is a 400 rather
/// than a silently ignored option.
#[utoipa::path(
    method(post),
    path = "/projects/{project_id}/query",
    params(
        ("project_id" = Uuid, Path, description = "Project UUID")
    ),
    request_body = QueryRequest,
    responses(
        (status = OK, description = "Columnar result set; `truncated` is true when the row cap was hit", body = QueryResponse, content_type = "application/json"),
        (status = BAD_REQUEST, description = "Empty SQL, an unknown field, or a statement the SELECT gate refused"),
        (status = FORBIDDEN, description = "Origin check or org membership failed")
    ),
    security(
        ("BearerAuth" = [])
    )
)]
#[instrument(skip_all, fields(project_id = %project_id))]
pub async fn run_query(
    Path(project_id): Path<Uuid>,
    uri: Uri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    // ── 1. Gates: auth → origin → user → DB → workspace → org-membership ──
    //
    // Body stays as raw bytes through the gate chain so the auth +
    // origin checks always run before any extractor-shaped error
    // leaks route existence or body schema to an unauthenticated
    // caller.
    let ctx = match check_custom_app_gates(&headers, project_id).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };

    // ── 2. Parse + validate body ─────────────────────────────────────────
    let req: QueryRequest = match parse_versioned_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if req.sql.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "`sql` must be non-empty");
    }

    // ── 3. Build workspace context ────────────────────────────────────────
    let proj_ctx = match ctx.build_project_context().await {
        Ok(ctx) => ctx,
        Err(resp) => return resp,
    };

    // ── 4. Resolve connector ──────────────────────────────────────────────
    // When the bundle doesn't pass `database`, fall back to the project's
    // configured default (`defaults.database` in config.yml) and then to
    // the first listed database. Empty-string lookup against
    // `resolve_database` would otherwise return ConfigurationError and
    // surface as an unhelpful 400 to the bundle.
    let db_name = match req.database.as_deref() {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => {
            let cm = &proj_ctx.workspace_manager().config_manager;
            if let Some(default) = cm.default_database_ref() {
                default.clone()
            } else if let Some(first) = cm.list_databases().first() {
                first.name.clone()
            } else {
                return err(
                    StatusCode::BAD_REQUEST,
                    "this project has no databases configured; add one in config.yml",
                );
            }
        }
    };
    let db_name = db_name.as_str();

    // ── 4a. Write the caller's params into the SQL ────────────────────────
    // By the rule of this database's engine, read from `config.yml`: no
    // connector is built to quote a value, and the cache below is keyed by
    // the SQL that will run. A database `config.yml` does not hold is
    // answered as step 4 would answer it, not as an engine nobody named —
    // every configured engine has a rule.
    let sql = match &req.params {
        Some(params) => {
            let config = &proj_ctx.workspace_manager().config_manager;
            let literal = match config.resolve_database(db_name) {
                Ok(database) => string_literal_of(&database.database_type),
                Err(OxyError::ConfigurationError(msg)) => {
                    return err(StatusCode::BAD_REQUEST, msg);
                }
                Err(e) => {
                    // Not the caller's to fix: the same class step 4 answers 500.
                    error!("resolve database '{db_name}' to quote params: {e}");
                    return err(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("could not read database '{db_name}' from config.yml"),
                    );
                }
            };
            match super::query_params::bind_params(&req.sql, params, literal) {
                Ok(bound) => bound,
                Err(why) => return err(StatusCode::BAD_REQUEST, why),
            }
        }
        None => req.sql,
    };
    let refresh = uri
        .query()
        .map(|q| {
            q.split('&')
                .any(|kv| kv == "refresh" || kv.starts_with("refresh="))
        })
        .unwrap_or(false);
    // `untyped` selects a different execution path (`execute_query_full_untyped`
    // returns every cell as text), so the two produce different bodies for the
    // same SQL. They must never share a cache entry or whichever caller writes
    // first wins and the other reads the wrong cell types. Namespace by the flag,
    // exactly as `semantic_query` does for its `?debug=1` shape.
    let cache_ns = if req.untyped {
        "query-untyped"
    } else {
        "query"
    };
    // A pinned staging request may resolve a different `config.yml` (its
    // branch's databases), so it gets its own partition.
    let cache_db = format!("{db_name}{}", ctx.cache_scope());
    if !refresh && let Some(body) = super::result_cache::get(project_id, cache_ns, &cache_db, &sql)
    {
        return (
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            (*body).clone(),
        )
            .into_response();
    }
    let connector = match proj_ctx.build_connector_for(db_name).await {
        Ok(c) => c,
        Err(OxyError::ConfigurationError(msg)) => {
            return err(StatusCode::BAD_REQUEST, msg);
        }
        Err(e) => {
            // The previous generic "could not resolve database 'X'" body
            // hid the real cause (connection refused / auth failure /
            // TLS error) inside the server log. Bundle authors hitting
            // a 500 had no actionable signal without `kubectl logs`.
            // Surface the underlying error directly — agentic_connector
            // formats these as host/protocol diagnostics (e.g.
            // "authentication failed for user 'oxy'", "connection
            // refused (os error 61)"); none contain secret values.
            error!("connector build failed for '{db_name}': {e}");
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to connect to database '{db_name}': {e}"),
            );
        }
    };

    // ── 5. Execute query ─────────────────────────────────────────────────
    match run_sql_query(connector, &sql, req.untyped).await {
        Ok(response) => {
            let bytes = match serde_json::to_vec(&response) {
                Ok(b) => b,
                Err(e) => {
                    error!("serialize query response: {e}");
                    return Json(response).into_response();
                }
            };
            let arc = std::sync::Arc::new(bytes);
            super::result_cache::put(project_id, cache_ns, &cache_db, &sql, arc.clone());
            (
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                (*arc).clone(),
            )
                .into_response()
        }
        Err(resp) => resp,
    }
}

/// Execute SQL via the connector and return the columnar `QueryResponse`.
///
/// Before executing:
///   - Rejects queries whose first token is not SELECT or WITH (Fix 2).
///   - Wraps the caller's SQL in an outer `LIMIT` subquery capped at
///     `MAX_ROWS` (Fix 3).
///
/// Error messages are sanitised before returning to the caller; the full
/// detail is logged server-side (Fix 4).
async fn run_sql_query(
    connector: Arc<dyn DatabaseConnector>,
    sql: &str,
    untyped: bool,
) -> Result<QueryResponse, Response> {
    // Gate: only SELECT / WITH are allowed through the proxy.
    if !is_read_only_sql(sql) {
        return Err(err(
            StatusCode::FORBIDDEN,
            "only SELECT/WITH queries are allowed via this endpoint",
        ));
    }

    // Wrap in an outer LIMIT so a `SELECT * FROM huge_table` cannot OOM the
    // server. If the caller already has a smaller LIMIT, the outer wrap is a
    // no-op ceiling — results are still correct.
    let limited_sql = wrap_with_limit(sql, MAX_ROWS);

    // `untyped` callers (the world-model dashboard, which already `::VARCHAR`-casts
    // every column) take the connector's fast path: skips airhouse's per-query
    // `DESCRIBE` round-trip and returns text cells. Everyone else stays typed.
    let query_result = if untyped {
        connector.execute_query_full_untyped(&limited_sql).await
    } else {
        connector.execute_query_full(&limited_sql).await
    };
    let stream = match query_result {
        Ok(s) => s,
        Err(ConnectorError::QueryFailed(detail)) => {
            warn!(detail = ?detail.message, "query proxy: query failed");
            return Err(err(
                StatusCode::BAD_REQUEST,
                "query failed; see server logs for details",
            ));
        }
        Err(ConnectorError::ConnectionError(msg)) => {
            error!(msg = ?msg, "query proxy: warehouse connection failed");
            return Err(err(StatusCode::BAD_GATEWAY, "warehouse connection failed"));
        }
        Err(ConnectorError::Other(msg)) => {
            error!(msg = ?msg, "query proxy: query execution error");
            return Err(err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "query execution failed",
            ));
        }
    };

    let (objects, connector_truncated) = match typed_stream_to_json_objects(stream).await {
        Ok(rows) => rows,
        Err(e) => {
            error!("row conversion failed: {e}");
            return Err(err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to convert query results",
            ));
        }
    };

    // Truncated if the soft row cap filled OR the connector hit its byte/row
    // backstop — the latter can stop *below* MAX_ROWS on wide rows, which the
    // length check alone would miss.
    //
    // `/semantic-query` fetches `MAX_ROWS + 1` and compares `>` to make this
    // exact. This endpoint is left as-is deliberately: changing its truncation
    // semantics was out of scope for that change, and the false positive on a
    // result that is exactly MAX_ROWS rows is conservative in the safe
    // direction. So the two endpoints can answer "is this complete?"
    // differently at the boundary — that divergence is known, not an oversight.
    let truncated = objects.len() == MAX_ROWS || connector_truncated;
    Ok(json_objects_to_table(objects, truncated))
}

// ── LIMIT wrapper ─────────────────────────────────────────────────────────────

/// Execute read-only SQL for an Oxy Function's `ctx.query`, returning the
/// rows as JSON objects capped at `max_rows`. Shares the read-only gate and
/// outer-LIMIT wrap with the custom-app `/query` endpoint so the safety
/// rules can't drift between the two call sites. Errors are returned as
/// human-readable strings (the function runtime surfaces them to the isolate
/// as a rejected promise).
pub(crate) async fn execute_function_query(
    connector: Arc<dyn DatabaseConnector>,
    sql: &str,
    max_rows: usize,
) -> Result<Vec<JsonValue>, String> {
    if !is_read_only_sql(sql) {
        return Err("only SELECT/WITH queries are allowed".to_string());
    }
    let limited_sql = wrap_with_limit(sql, max_rows);
    let stream = connector
        .execute_query_full(&limited_sql)
        .await
        .map_err(|e| match e {
            ConnectorError::QueryFailed(detail) => {
                format!("query failed: {}", detail.message)
            }
            ConnectorError::ConnectionError(msg) => format!("warehouse connection failed: {msg}"),
            ConnectorError::Other(msg) => format!("query execution failed: {msg}"),
        })?;
    // `typed_stream_to_json_objects` returns `(rows, connector_truncated)`; the
    // function runtime tracks truncation itself via a +1 overfetch
    // (`host::query_with_truncation`), so drop the flag here and return the rows.
    typed_stream_to_json_objects(stream)
        .await
        .map(|(objects, _truncated)| objects)
        .map_err(|e| format!("failed to convert query results: {e}"))
}

/// Production
/// [`FunctionQueryExecutor`](crate::server::api::custom_apps_functions::seam::FunctionQueryExecutor)
/// — delegates to [`execute_function_query`] so the read-only gate and
/// outer-LIMIT wrap stay shared with the custom-app `/query` endpoint.
///
/// Stateless: constructed at the composition root (the serve router / the
/// scheduled-function worker) and handed to the function runtime as a trait
/// object, so the runtime never imports this module. See
/// `custom_apps_functions::seam::FunctionQueryExecutor`.
pub struct DataPlaneQueryExecutor;

#[async_trait::async_trait]
impl crate::server::api::custom_apps_functions::seam::FunctionQueryExecutor
    for DataPlaneQueryExecutor
{
    async fn execute(
        &self,
        connector: Arc<dyn DatabaseConnector>,
        sql: &str,
        max_rows: usize,
    ) -> Result<Vec<JsonValue>, String> {
        execute_function_query(connector, sql, max_rows).await
    }
}

/// Wrap `sql` in `SELECT * FROM (...) AS oxy_proxy_query LIMIT {max_rows}`.
///
/// Trailing semicolons (and surrounding whitespace) are stripped first so that
/// `SELECT 1;` doesn't produce `SELECT * FROM (SELECT 1;) AS …`, which most
/// warehouses reject.
fn wrap_with_limit(sql: &str, max_rows: usize) -> String {
    let sql_trimmed = sql.trim_end().trim_end_matches(';').trim_end();
    format!("SELECT * FROM (\n{sql_trimmed}\n) AS oxy_proxy_query LIMIT {max_rows}")
}

// ── SELECT/WITH guard ─────────────────────────────────────────────────────────

/// Returns `true` if the SQL starts (modulo leading whitespace and `--`/`/* */`
/// comments) with SELECT or WITH **and** contains no statement terminator
/// after stripping string literals. Defense against bundles trying to execute
/// writes through the read proxy.
///
/// The first-token check alone is bypassable via multi-statement input like
/// `SELECT 1; DROP TABLE x`. Some warehouses (DuckDB at least) parse multiple
/// statements when given them. The outer-LIMIT wrap in `wrap_with_limit`
/// makes this unreachable for warehouses that disallow subquery-internal
/// semicolons (BigQuery, Snowflake), but DuckDB and ClickHouse have their
/// own multi-statement semantics — we don't want to depend on per-warehouse
/// parser quirks for security. So: reject any unquoted `;` in the SQL after
/// string literals are stripped.
fn is_read_only_sql(sql: &str) -> bool {
    let stripped = strip_leading_comments_and_ws(sql);
    let first_token: String = stripped
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if !matches!(first_token.to_ascii_uppercase().as_str(), "SELECT" | "WITH") {
        return false;
    }
    !contains_statement_terminator(sql)
}

/// Returns `true` if `sql` contains a `;` that is NOT inside a string literal.
/// Trailing `;` is also rejected — `wrap_with_limit` strips trailing
/// semicolons before formatting, but a caller's `SELECT 1; foo` slips past
/// `trim_end_matches` because of the trailing content. So reject any `;`
/// at all (outside literals).
///
/// String literal recognition: SQL single-quoted strings with `''` escape
/// (standard), plus PostgreSQL-style dollar-quoted strings (`$tag$ ... $tag$`).
/// Conservative — false positives (rejecting weird-but-valid SQL) are
/// acceptable; false negatives (accepting a `;` outside the gate) are not.
fn contains_statement_terminator(sql: &str) -> bool {
    let bytes = sql.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b';' {
            return true;
        }
        if b == b'\'' {
            // Skip a single-quoted string, honoring '' as the literal-quote escape.
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\'' {
                    if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if b == b'$' {
            // Dollar-quoted: scan an optional tag, then look for the
            // matching `$tag$ ... $tag$`. Tags are `[A-Za-z_][A-Za-z0-9_]*`.
            let tag_start = i + 1;
            let mut tag_end = tag_start;
            while tag_end < bytes.len() {
                let c = bytes[tag_end];
                let valid = if tag_end == tag_start {
                    c.is_ascii_alphabetic() || c == b'_'
                } else {
                    c.is_ascii_alphanumeric() || c == b'_'
                };
                if !valid {
                    break;
                }
                tag_end += 1;
            }
            if tag_end < bytes.len() && bytes[tag_end] == b'$' {
                let tag = &bytes[tag_start..tag_end];
                let closing_start = tag_end + 1;
                let mut j = closing_start;
                while j + tag.len() + 1 < bytes.len() {
                    if bytes[j] == b'$'
                        && bytes[j + 1 + tag.len()] == b'$'
                        && &bytes[j + 1..j + 1 + tag.len()] == tag
                    {
                        i = j + 2 + tag.len();
                        break;
                    }
                    j += 1;
                }
                if i <= tag_end {
                    // No matching close — treat the rest as inside the literal.
                    return false;
                }
                continue;
            }
            // Not a dollar-quoted opener — just a bare `$`, fall through.
        }
        if b == b'-' && i + 1 < bytes.len() && bytes[i + 1] == b'-' {
            // Line comment — skip to newline.
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            // Block comment — skip to closing delimiter.
            i += 2;
            while i + 1 < bytes.len() {
                if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    i += 2;
                    break;
                }
                i += 1;
            }
            continue;
        }
        i += 1;
    }
    false
}

fn strip_leading_comments_and_ws(mut s: &str) -> &str {
    loop {
        s = s.trim_start();
        if let Some(rest) = s.strip_prefix("--") {
            // line comment — skip to next newline
            if let Some(nl) = rest.find('\n') {
                s = &rest[nl + 1..];
            } else {
                return "";
            }
        } else if let Some(rest) = s.strip_prefix("/*") {
            // block comment — skip to closing delimiter
            if let Some(end) = rest.find("*/") {
                s = &rest[end + 2..];
            } else {
                return "";
            }
        } else {
            return s;
        }
    }
}

// ── Reshape helper ────────────────────────────────────────────────────────────

/// Convert `Vec<JsonValue>` (each value is a `{col: value}` object) into the
/// columnar `{ columns, rows, truncated }` shape.
///
/// Column order is determined by first-seen key order across all rows;
/// subsequent rows that are missing a key get `JsonValue::Null` in that
/// column's slot.
pub fn json_objects_to_table(objects: Vec<JsonValue>, truncated: bool) -> QueryResponse {
    // Build a stable column index using an IndexMap (preserves insertion order).
    let mut col_index: IndexMap<String, usize> = IndexMap::new();

    for obj in &objects {
        if let Some(map) = obj.as_object() {
            for key in map.keys() {
                let next = col_index.len();
                col_index.entry(key.clone()).or_insert(next);
            }
        }
    }

    let columns: Vec<String> = col_index.keys().cloned().collect();
    let col_count = columns.len();

    let rows: Vec<Vec<JsonValue>> = objects
        .into_iter()
        .map(|obj| {
            let mut row = vec![JsonValue::Null; col_count];
            if let Some(map) = obj.as_object() {
                for (k, v) in map {
                    if let Some(&idx) = col_index.get(k) {
                        row[idx] = v.clone();
                    }
                }
            }
            row
        })
        .collect();

    QueryResponse {
        columns,
        rows,
        truncated,
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn json_objects_to_table_aligned_columns() {
        let objects = vec![json!({ "a": 1, "b": "x" }), json!({ "a": 2, "b": "y" })];
        let table = json_objects_to_table(objects, false);
        assert_eq!(table.columns, vec!["a", "b"]);
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[0], vec![json!(1), json!("x")]);
        assert_eq!(table.rows[1], vec![json!(2), json!("y")]);
        assert!(!table.truncated);
    }

    #[test]
    fn json_objects_to_table_empty_input() {
        let table = json_objects_to_table(vec![], false);
        assert!(table.columns.is_empty());
        assert!(table.rows.is_empty());
        assert!(!table.truncated);
    }

    #[test]
    fn json_objects_to_table_missing_cells_become_null() {
        let objects = vec![json!({ "a": 1 }), json!({ "a": 2, "b": "hello" })];
        let table = json_objects_to_table(objects, false);
        assert_eq!(table.columns, vec!["a", "b"]);
        // First row is missing "b"
        assert_eq!(table.rows[0], vec![json!(1), JsonValue::Null]);
        assert_eq!(table.rows[1], vec![json!(2), json!("hello")]);
    }

    #[test]
    fn json_objects_to_table_truncated_flag_propagated() {
        let table = json_objects_to_table(vec![json!({ "a": 1 })], true);
        assert!(table.truncated);
    }

    #[test]
    fn query_request_takes_params_and_runs_without_them() {
        let with: QueryRequest =
            serde_json::from_str(r#"{"sql":"x","params":{"a":"v","n":1,"z":null}}"#).unwrap();
        assert_eq!(
            with.params.map(JsonValue::Object),
            Some(json!({ "a": "v", "n": 1, "z": null }))
        );
        // What every SDK before `params` sends.
        let without: QueryRequest = serde_json::from_str(r#"{"sql":"x"}"#).unwrap();
        assert!(without.params.is_none());
    }

    #[test]
    fn query_request_rejects_unknown_fields() {
        let r: Result<QueryRequest, _> = serde_json::from_str(r#"{"sql":"x","extra":1}"#);
        assert!(r.is_err(), "unknown field `extra` should be rejected");
    }

    // ── SELECT gate tests ─────────────────────────────────────────────────

    #[test]
    fn is_read_only_sql_plain_select() {
        assert!(is_read_only_sql("SELECT 1"));
    }

    #[test]
    fn is_read_only_sql_lowercase_select() {
        assert!(is_read_only_sql("select * from t"));
    }

    #[test]
    fn is_read_only_sql_with_clause() {
        assert!(is_read_only_sql("WITH cte AS (SELECT 1) SELECT * FROM cte"));
    }

    #[test]
    fn is_read_only_sql_leading_whitespace() {
        assert!(is_read_only_sql("   \t\n SELECT 1"));
    }

    #[test]
    fn is_read_only_sql_leading_line_comment() {
        assert!(is_read_only_sql("-- get totals\nSELECT count(*) FROM t"));
    }

    #[test]
    fn is_read_only_sql_leading_block_comment() {
        assert!(is_read_only_sql("/* analytics */ SELECT 1"));
    }

    #[test]
    fn is_read_only_sql_mixed_comments_and_ws() {
        assert!(is_read_only_sql(
            "-- line\n/* block */ \n WITH x AS (SELECT 1) SELECT * FROM x"
        ));
    }

    #[test]
    fn is_read_only_sql_rejects_delete() {
        assert!(!is_read_only_sql("DELETE FROM t WHERE id = 1"));
    }

    #[test]
    fn is_read_only_sql_rejects_insert() {
        assert!(!is_read_only_sql("INSERT INTO t VALUES (1)"));
    }

    #[test]
    fn is_read_only_sql_rejects_drop() {
        assert!(!is_read_only_sql("DROP TABLE t"));
    }

    #[test]
    fn is_read_only_sql_rejects_update() {
        assert!(!is_read_only_sql("UPDATE t SET x = 1"));
    }

    #[test]
    fn is_read_only_sql_rejects_multi_statement() {
        // `SELECT 1; DROP TABLE x` passes the first-token check but fails
        // the statement-terminator check. The outer-LIMIT subquery wrap
        // makes the DROP unreachable on warehouses that reject multi-
        // statement subqueries (BigQuery, Snowflake), but DuckDB and
        // ClickHouse have permissive parsers — we reject at the gate so
        // security doesn't depend on per-warehouse quirks.
        assert!(!is_read_only_sql("SELECT 1; DROP TABLE x"));
    }

    #[test]
    fn is_read_only_sql_rejects_trailing_semicolon() {
        // `SELECT 1;` is a single statement but the trailing `;` is rejected
        // to keep the rule simple ("no unquoted `;` at all"). `wrap_with_limit`
        // strips trailing semicolons defensively anyway, so callers don't
        // need to worry about them — but the gate stays strict.
        assert!(!is_read_only_sql("SELECT 1;"));
    }

    #[test]
    fn is_read_only_sql_allows_semicolon_inside_string_literal() {
        // Semicolons inside string literals are not statement terminators —
        // gate must not reject legitimate SQL like `WHERE name = 'a;b'`.
        assert!(is_read_only_sql("SELECT * FROM t WHERE name = 'a;b'"));
    }

    #[test]
    fn is_read_only_sql_allows_escaped_single_quote_in_string() {
        // Standard SQL: '' is the escape for a single quote inside a string
        // literal. `'a''b;c'` is a single string containing `a'b;c` — the `;`
        // is inside the literal and must not trip the terminator check.
        assert!(is_read_only_sql("SELECT * FROM t WHERE name = 'a''b;c'"));
    }

    #[test]
    fn is_read_only_sql_allows_semicolon_inside_dollar_quoted_string() {
        // PostgreSQL dollar-quoted strings: `$tag$ ... $tag$`. Semicolons
        // inside are part of the literal, not statement terminators.
        assert!(is_read_only_sql(
            "SELECT $foo$ has ; semicolons $foo$ AS literal"
        ));
    }

    #[test]
    fn is_read_only_sql_rejects_semicolon_after_string_literal() {
        // A string literal closes; then a real terminator follows.
        assert!(!is_read_only_sql("SELECT 'safe'; DROP TABLE x"));
    }

    #[test]
    fn is_read_only_sql_allows_semicolon_inside_block_comment() {
        // Block comments can contain anything including `;`.
        assert!(is_read_only_sql("SELECT 1 /* harmless ; comment */ FROM t"));
    }

    #[test]
    fn is_read_only_sql_allows_semicolon_inside_line_comment() {
        // Line comments terminate at newline; `;` inside a line comment is
        // not a statement terminator.
        assert!(is_read_only_sql("SELECT 1 -- harmless ; comment\nFROM t"));
    }

    // ── wrap_with_limit tests ─────────────────────────────────────────────

    #[test]
    fn limit_wrap_strips_trailing_semicolon() {
        let wrapped = wrap_with_limit("SELECT 1;", 10000);
        assert!(
            wrapped.contains("SELECT 1\n"),
            "semicolon should be stripped"
        );
        assert!(
            !wrapped.contains("SELECT 1;"),
            "raw semicolon must not appear before closing paren"
        );
    }

    #[test]
    fn limit_wrap_strips_trailing_whitespace_and_semicolon() {
        // "SELECT 1 ;" — space between statement and semicolon
        let wrapped = wrap_with_limit("SELECT 1 ;", 10000);
        assert!(!wrapped.contains(";"), "semicolon must be stripped");
    }

    #[test]
    fn limit_wrap_clean_query_unchanged() {
        let wrapped = wrap_with_limit("SELECT 1", 10000);
        assert!(wrapped.starts_with("SELECT * FROM (\nSELECT 1\n)"));
    }

    #[test]
    fn limit_wrap_applies_max_rows() {
        let wrapped = wrap_with_limit("SELECT 1", 42);
        assert!(wrapped.ends_with("LIMIT 42"));
    }
}
