//! `GET …/analytics/runs/{id}/results/{result_id}`: one page of a react run's
//! stored query result, for the answer's table to load past its first rows.
//!
//! Readable by the run's workspace, as the run's events are. Thin: the key
//! checks, paging and expiry are `agentic_runtime::react`'s; storage is the
//! platform's (`PlatformContext::result_archive`).

use std::sync::Arc;

use agentic_pipeline::platform::PlatformContext;
use agentic_runtime::react::{ArchiveError, ArchiveKey, PageRequest, page};
use axum::{
    Json,
    extract::{Extension, Path, Query},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;

use super::run_scope::run_in_workspace;
use crate::state::AgenticState;

/// Rows per page when the caller names none.
const DEFAULT_PAGE_ROWS: usize = 100;

#[derive(Deserialize)]
pub struct ResultPath {
    id: String,
    result_id: String,
}

#[derive(Deserialize)]
pub struct ResultPageQuery {
    offset: Option<usize>,
    limit: Option<usize>,
    /// The column's place in the result, from 0.
    sort: Option<usize>,
    desc: Option<bool>,
}

pub async fn get_result_page(
    Path(ResultPath { id, result_id }): Path<ResultPath>,
    Query(q): Query<ResultPageQuery>,
    Extension(state): Extension<Arc<AgenticState>>,
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
) -> Response {
    let workspace_id = platform.workspace_id();
    if let Err(resp) = run_in_workspace(&state.db, &id, workspace_id).await {
        return resp;
    }
    let req = PageRequest {
        offset: q.offset.unwrap_or(0),
        limit: q.limit.unwrap_or(DEFAULT_PAGE_ROWS),
        sort: q.sort,
        desc: q.desc.unwrap_or(false),
    };
    let found = match (
        ArchiveKey::new(workspace_id, &id, &result_id),
        platform.result_archive(),
    ) {
        (Err(e), _) => return refusal(e),
        (Ok(_), None) => return refusal(ArchiveError::NotFound),
        (Ok(key), Some(archive)) => archive.get(&key).await,
    };
    match found.and_then(|r| r.ok_or(ArchiveError::NotFound)) {
        Ok(result) => match page(&result, &req, chrono::Utc::now()) {
            Ok(p) => Json(p).into_response(),
            Err(e) => refusal(e),
        },
        Err(e) => refusal(e),
    }
}

/// 400 for a bad id or sort column, 404 when nothing is stored, 410 with the
/// SQL to re-run once expired, 500 when storage failed.
fn refusal(e: ArchiveError) -> Response {
    let (status, body) = match &e {
        ArchiveError::BadRequest(m) => (StatusCode::BAD_REQUEST, serde_json::json!({"error": m})),
        ArchiveError::NotFound => (
            StatusCode::NOT_FOUND,
            serde_json::json!({"error": "result not found"}),
        ),
        ArchiveError::Expired { sql } => (
            StatusCode::GONE,
            serde_json::json!({"error": "result expired", "sql": sql}),
        ),
        ArchiveError::Storage(m) => {
            tracing::warn!("react result read failed: {m}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": "result storage"}),
            )
        }
    };
    (status, Json(body)).into_response()
}
