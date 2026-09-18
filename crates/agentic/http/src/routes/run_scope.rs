//! A run id names a run in ONE workspace.
//!
//! Every `/runs/{id}/…` route is mounted under `/{workspace_id}/…`, where the
//! workspace middleware has already checked that the caller belongs to that
//! workspace. The run id in the path is still the caller's to choose, so each
//! handler must also check that the run IS that workspace's — otherwise a
//! member of one workspace reads another's run events, answers its suspended
//! run, or cancels it, given only the id. A run from another workspace answers
//! 404, exactly like a run that does not exist: its existence is not ours to
//! confirm.
//!
//! `PlatformContext::workspace_id()` is the id the run was stamped with at
//! submit (the nil UUID in local mode), so the check holds in every serve mode.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use crate::state::AgenticState;

/// The run, when it exists in `workspace_id`; the 404 or 500 to send otherwise.
pub(super) async fn run_in_workspace(
    db: &DatabaseConnection,
    run_id: &str,
    workspace_id: Uuid,
) -> Result<agentic_runtime::entity::run::Model, Response> {
    match agentic_runtime::crud::get_run(db, run_id).await {
        Ok(Some(run)) if run.workspace_id == workspace_id => Ok(run),
        Ok(_) => Err((StatusCode::NOT_FOUND, "run not found").into_response()),
        Err(e) => {
            Err((StatusCode::INTERNAL_SERVER_ERROR, format!("db error: {e}")).into_response())
        }
    }
}

/// Verify `user_id` may operate on `run_id` in `workspace_id`:
///
/// - `404` if the run does not exist, or exists in another workspace
/// - `403` if the run is linked to a thread owned by another user
/// - `500` on a lookup failure
///
/// Runs without a `thread_id` (scheduled, manual and background runs — the
/// common case for airway and automation) are open to the workspace's members.
/// Single chokepoint for the automation and airway run routes.
pub(super) async fn ensure_run_access(
    state: &AgenticState,
    user_id: &Uuid,
    run_id: &str,
    workspace_id: Uuid,
) -> Result<(), Response> {
    let run = run_in_workspace(&state.db, run_id, workspace_id).await?;
    let Some(thread_uuid) = run.thread_id else {
        return Ok(());
    };
    match state.thread_owner.thread_owner(thread_uuid).await {
        // The run's thread was deleted out from under it: treat as missing
        // rather than leaking the run row.
        Ok(None) => Err((StatusCode::NOT_FOUND, "run not found").into_response()),
        Ok(Some(Some(owner_id))) if &owner_id != user_id => {
            Err((StatusCode::FORBIDDEN, "access denied").into_response())
        }
        Ok(_) => Ok(()),
        Err(e) => {
            Err((StatusCode::INTERNAL_SERVER_ERROR, format!("db error: {e}")).into_response())
        }
    }
}
