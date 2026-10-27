//! `POST /runs/{id}/notify`: tell me when this run ends.
//!
//! An answer takes long enough that people leave the app meanwhile. A client
//! calls this while the run is going — the app went to the background, or the
//! person closed Ask — and the run's thread owner gets a notice when it ends.
//! Nothing is sent to someone who stayed and watched. The request is recorded
//! on the run; when it has already ended, the notice goes now
//! (`agentic_runtime::lifecycle::notice_request`). Calling it twice is fine.
//!
//! Only the owner of the run's thread may ask: the notice goes to that owner,
//! and anyone else learns nothing, so a run in another workspace, a run with
//! no thread, and a run on someone else's thread all answer like a run that
//! does not exist.

use std::sync::Arc;

use agentic_pipeline::platform::PlatformContext;
use axum::{
    Json,
    extract::{Extension, Path},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use uuid::Uuid;

use super::RunIdPath;
use super::run_scope::run_in_workspace;
use crate::state::AgenticState;

pub async fn request_run_notice(
    Path(RunIdPath { id: run_id }): Path<RunIdPath>,
    Extension(state): Extension<Arc<AgenticState>>,
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
) -> Response {
    if let Err(refused) = own_thread_run(&state, &run_id, platform.workspace_id(), user.id).await {
        return refused;
    }
    match agentic_runtime::lifecycle::notice_request::request_notice(&state.db, &run_id).await {
        Ok(true) => Json(serde_json::json!({})).into_response(),
        Ok(false) => not_found(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("db error: {e}")).into_response(),
    }
}

/// `Ok` when `run_id` is in `workspace_id` and its thread is `user_id`'s;
/// otherwise the `404` (or a lookup's `500`) to send.
async fn own_thread_run(
    state: &AgenticState,
    run_id: &str,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<(), Response> {
    let run = run_in_workspace(&state.db, run_id, workspace_id).await?;
    let Some(thread_id) = run.thread_id else {
        return Err(not_found());
    };
    match state.thread_owner.thread_owner(thread_id).await {
        Ok(Some(Some(owner))) if owner == user_id => Ok(()),
        Ok(_) => Err(not_found()),
        Err(e) => {
            Err((StatusCode::INTERNAL_SERVER_ERROR, format!("db error: {e}")).into_response())
        }
    }
}

/// The answer `run_in_workspace` gives a run that does not exist.
fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "run not found").into_response()
}
