//! What an interactive airway **request** does that a queue claim must not.
//!
//! Start, single-window backfill and both resets may be served by any replica,
//! including one with no working copy. There the pipeline's YAML comes from the
//! compile boundary or not at all, and "not at all" has two shapes
//! ([`PipelineRefError`]): nobody could be asked (`Unavailable`), and the
//! promoted revision was asked and does not serve the ref (`NotInRevision`).
//!
//! Only the second is fixed by a compile, and only a **mutating** request should
//! ask for one. The same load runs at queue claim (`executor::execute_airway`,
//! every 30s while deferred) and on every schedule tick (`scheduler`); a
//! compile requested from those would mint a fresh revision per repeat for a
//! ref that is simply gone. So this module is the one place
//! [`WorkspaceContext::request_compile`] is called, and the picker read
//! (`GET /resource-cursors`, `executor::cursor_reset`) does not come through it.
//!
//! A request is still not person-rate: a React Query fetch retries a 503 three
//! times, a stale tab refetches, any client honouring `Retry-After` comes back
//! on schedule, and a self-heal compile that succeeds and still does not serve
//! the ref leaves the next request asking again. The host bounds that side —
//! `oxy-app`'s `request_compile` refuses within a window of a successful
//! compile and then answers `false`, so the message below is not appended.
//!
//! On a node that holds a working copy none of this is reachable:
//! `NotInRevision` is only ever produced with no working copy to read, so the
//! IDE, a single-process deployment and the CLI behave exactly as before.

use agentic_automation::WorkspaceContext;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use crate::airway_run::{AirwayRunError, StartAirwayRequest, start_airway_run};
use crate::pipeline_ref::{PipelineRefError, load_pipeline_yaml};

/// Ask the host for a compile after a `NotInRevision`, and say so.
///
/// Returns the message to hand the caller. It claims a compile was *requested*
/// only when the host took the request, and never that one ran: the host
/// dedupes and backs off, and on a fleet whose only compiling node is down the
/// task waits in the queue until it is back.
async fn request_compile_for(workspace: &dyn WorkspaceContext, message: String) -> String {
    if workspace.request_compile().await {
        format!("{message} A compile has been requested; retry shortly.")
    } else {
        message
    }
}

/// [`load_pipeline_yaml`] for an interactive request.
///
/// Identical except that a `NotInRevision` asks the host for a compile first,
/// so the retry the caller is told to make has something to wait for.
pub async fn load_pipeline_yaml_for_request(
    workspace: &dyn WorkspaceContext,
    pipeline_ref: &str,
) -> Result<String, PipelineRefError> {
    match load_pipeline_yaml(workspace, pipeline_ref).await {
        Err(PipelineRefError::NotInRevision(m)) => Err(PipelineRefError::NotInRevision(
            request_compile_for(workspace, m).await,
        )),
        other => other,
    }
}

/// [`start_airway_run`] for an interactive submit (`POST /runs`, `/backfill`).
///
/// Enqueues `TaskScope::Global` — the worker fleet drives the run, never the
/// node that accepted the submit — and asks for a compile when the promoted
/// revision does not serve the ref. Schedules, retries and Oxy Functions keep
/// calling `start_airway_run` directly, for the reason in the module doc.
pub async fn submit_airway_run(
    db: &DatabaseConnection,
    workspace: &dyn WorkspaceContext,
    request: StartAirwayRequest,
    workspace_id: Uuid,
) -> Result<String, AirwayRunError> {
    match start_airway_run(
        db,
        workspace,
        request,
        crate::TaskScope::Global,
        workspace_id,
    )
    .await
    {
        Err(AirwayRunError::NotInRevision(m)) => Err(AirwayRunError::NotInRevision(
            request_compile_for(workspace, m).await,
        )),
        other => other,
    }
}

#[cfg(test)]
#[path = "airway_request_tests.rs"]
mod tests;
