//! `POST /api/projects/{project_id}/agents/asks/{run_id}/cancel`
//!
//! Stop an in-flight ask. Idempotent, 204 either way.
//!
//! The replica that takes the cancel is often not the process driving the
//! run, so this handler does three things, in the order that makes the later
//! ones safe:
//!
//! 1. **Writes the durable flag** (`agentic_runs.cancel_requested_at`). It is
//!    what reaches a driver in another process: a run recovery is driving
//!    polls it every 5 s and tears its task tree down.
//! 2. **Signals this process's driver**, when there is one. The fast path:
//!    the ask's own handler registered a cancel channel here, or recovery
//!    did.
//! 3. **Fails the row only when nothing can be driving it and it has not
//!    ended.** A run that already finished stays as it finished; a run a
//!    live driver holds — a fresh driver lease, or a queue entry a driver
//!    holds or is about to take — is left to that driver, which turns the
//!    flag into a proper `cancelled`. What is left is a run nobody will ever
//!    close, and failing it is what keeps it from reading `running` for ever.
//!
//! **An ask its handler is driving on another replica** holds the driver lease
//! (`drive_with_coordinator` takes it through
//! `agentic_runtime::lifecycle::driver_lease`), so step 3 leaves it alone and
//! the flag from step 1 reaches it: that lease's ticker polls the flag and
//! fires the driving replica's cancel channel within about 5 s.
//!
//! **On an app's staging host** it is the same decision as the ask
//! ([`ask_scope`]), and it cancels only a run that app's staging ask started
//! ([`staging_run_matches`]); anything else is the same 404 as a missing run.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use tracing::{error, instrument};
use uuid::Uuid;

use super::super::agent_ask_staging::{ask_scope, staging_run_matches};
use super::{boxed, err, err_with_code};
use crate::server::api::custom_apps_gates::check_custom_app_gates;
use crate::server::router::AppState;

/// The `error_message` of a run this endpoint closed.
const CANCELLED_BY_USER: &str = "cancelled by user";

#[instrument(skip_all, fields(project_id = %project_id, run_id = %run_id))]
pub async fn cancel_ask(
    State(app_state): State<AppState>,
    Path((project_id, run_id)): Path<(Uuid, String)>,
    headers: HeaderMap,
) -> Response {
    let gates_ctx = match boxed(|| check_custom_app_gates(&headers, project_id)).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let hold = match boxed(|| ask_scope(&gates_ctx.db, &headers, &gates_ctx.user, project_id)).await
    {
        Ok(hold) => hold,
        Err(resp) => return resp,
    };
    let agentic_state = match app_state.agentic_state.as_ref() {
        Some(s) => s.clone(),
        None => {
            return err(
                StatusCode::SERVICE_UNAVAILABLE,
                "agent runtime not configured in this deployment",
            );
        }
    };
    let db = &agentic_state.db;

    // The run is resolved WITHIN the project the gates admitted the caller
    // to: another project's run, and a check run staff queued in this one
    // outside production, are the same not-found as an id that names nothing.
    // A member holding an id can neither confirm that run nor cancel it.
    let run = match agentic_runtime::crud::get_run_in_workspace(db, project_id, &run_id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return err_with_code(
                StatusCode::NOT_FOUND,
                "run not found",
                "agent_run_not_found",
            );
        }
        Err(e) => {
            error!(run_id = %run_id, error = %e, "cancel: run lookup failed");
            return err(StatusCode::INTERNAL_SERVER_ERROR, "run lookup failed");
        }
    };
    if let Some(hold) = &hold
        && !staging_run_matches(hold, run.metadata.as_ref())
    {
        return err_with_code(
            StatusCode::NOT_FOUND,
            "run not found",
            "agent_run_not_found",
        );
    }

    // Before the in-process signal: a driver in another process observes only
    // this, and a failure to write it is a cancel that reached nobody.
    if let Err(e) = agentic_runtime::crud::request_cancel(db, &run_id).await {
        error!(run_id = %run_id, error = %e, "cancel: could not record the cancel request");
        return err(StatusCode::INTERNAL_SERVER_ERROR, "cancel failed");
    }
    if agentic_state.runtime.cancel(&run_id) {
        return StatusCode::NO_CONTENT.into_response();
    }
    if let Err(e) =
        agentic_pipeline::queued_answer::close_if_undriven(db, &run_id, CANCELLED_BY_USER).await
    {
        error!(run_id = %run_id, error = %e, "cancel: closing an undriven run failed");
        return err(StatusCode::INTERNAL_SERVER_ERROR, "cancel failed");
    }
    StatusCode::NO_CONTENT.into_response()
}
