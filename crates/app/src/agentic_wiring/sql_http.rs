//! The HTTP answer of a Dev Portal SQL execute: the structured error body, the
//! status a failure maps to (a warehouse that is down is `502`, a query that is
//! wrong `400`, a statement a workspace preview held `409 preview_read_only`),
//! and the one call the `/sql` handlers make.
//!
//! Beside [`super::sql_exec`], which runs the query, since 2026-11-16 (Wave D2).
//! It was `api::data`'s, kept there by D1 as the handlers' own shaping. It came
//! down when the handlers left for `oxy-api-semantic`, because two callers
//! outside that crate need the exact answer a handler gives:
//! `oxy-api-previews`' `read_only_exec` test drives the preview write hold
//! through the real workspace middleware and asserts the `409` this mapping
//! produces, and may not depend on a sibling surface (S3). The semantic execute
//! (`oxy-api-semantic`'s `semantic`) answers a failed execute in the same shape.

use axum::extract;
use axum::http::StatusCode;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::{ConfigManager, DiskSlot, ResolveWorkspaceFile};
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

use agentic_connector::{ConnectorError, QueryFailedDetails};
use entity::workspace_members::WorkspaceRole;

use super::sql_exec::{
    SQLParams, SemanticQueryResponse, SqlExecuteError, run_via_agentic_connector,
};

/// Run `payload` the way `POST /{workspace_id}/sql/{pathb64}` and `POST
/// /{workspace_id}/sql/query` do, and answer as they answer: the rows, or the
/// status and structured body [`agentic_error_response`] shapes the failure
/// into. The handlers are this call plus their extractors.
pub async fn execute_sql_response<S: DiskSlot>(
    workspace_manager: &WorkspaceManager<S>,
    user_id: Uuid,
    role: WorkspaceRole,
    payload: &SQLParams,
) -> Result<extract::Json<SemanticQueryResponse>, (StatusCode, extract::Json<SqlErrorResponse>)>
where
    ConfigManager<S>: ResolveWorkspaceFile,
{
    run_via_agentic_connector(workspace_manager, user_id, role, payload)
        .await
        .map(extract::Json)
        .map_err(|e| agentic_error_response(payload, e))
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

/// Shape an `SqlExecuteError` into a (status, body) pair. Structured fields
/// from `ConnectorError::QueryFailed(details)` propagate; everything else
/// degrades to a single-line `message`.
pub fn agentic_error_response(
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
    // (decoder errors, internal driver bugs) is 500. A statement a workspace
    // preview held (`oxy_app_core::write_hold::connector`) is the preview's `409 preview_read_only`:
    // nothing failed, and nothing was sent.
    let (status, body) = match err {
        SqlExecuteError::Connector(ConnectorError::QueryFailed(d))
            if d.code.as_deref() == Some(oxy_app_core::write_hold::connector::HELD_CODE) =>
        {
            (
                StatusCode::CONFLICT,
                SqlErrorResponse {
                    message: d.message,
                    code: d.code,
                    detail: None,
                    hint: None,
                    position: None,
                    sql: None,
                },
            )
        }
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
