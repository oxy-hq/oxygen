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
//! **What this cannot see.** An ask its *handler* is driving on another
//! replica holds neither a lease nor a queue entry, so from here it reads as
//! undriven: the row is failed while that replica keeps going. That is the
//! one case left, and it is closed by moving the ask onto the task queue
//! (`internal-docs/worker-fleet.md` § "Custom-app runs on the queue"), where
//! every driven ask holds both.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use sea_orm::DatabaseConnection;
use tracing::{error, instrument, warn};
use uuid::Uuid;

use super::{err, err_with_code};
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
    let _gates_ctx = match check_custom_app_gates(&headers, project_id).await {
        Ok(c) => c,
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
    match agentic_runtime::crud::get_run_in_workspace(db, project_id, &run_id).await {
        Ok(Some(_)) => {}
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
    if let Err(e) = close_if_undriven(db, &run_id).await {
        error!(run_id = %run_id, error = %e, "cancel: closing an undriven run failed");
        return err(StatusCode::INTERNAL_SERVER_ERROR, "cancel failed");
    }
    StatusCode::NO_CONTENT.into_response()
}

/// Fail the run if nothing can be driving it and it has not ended; otherwise
/// write nothing. Returns whether the row was closed here.
///
/// The queue entry is read first and the row is closed by one guarded
/// statement (`fail_undriven_run`: not terminal, no live lease), so a run that
/// finishes or is claimed between the two is still never written over.
async fn close_if_undriven(db: &DatabaseConnection, run_id: &str) -> Result<bool, sea_orm::DbErr> {
    match agentic_runtime::crud::get_queue_entry(db, run_id).await {
        Ok(Some(entry)) if matches!(entry.queue_status.as_str(), "queued" | "claimed") => {
            return Ok(false);
        }
        Ok(_) => {}
        // Unknown. The flag is already written and is what stops a live run;
        // a wrong `failed` on one is not recoverable, so do not write.
        Err(e) => {
            warn!(%run_id, error = %e, "cancel: queue lookup failed; leaving the run row alone");
            return Ok(false);
        }
    }
    agentic_runtime::crud::fail_undriven_run(db, run_id, CANCELLED_BY_USER).await
}
