//! `ProcedureRunExecutor`: runs a queued custom-app procedure run
//! (`TaskSpec::Custom { kind: "custom_app_procedure_run" }`) on whichever
//! process claimed it. Registered into the `CustomTaskRegistry` by
//! `server::router::recovery`; `PipelineTaskExecutor` delegates the kind here.
//!
//! The work itself is unchanged — the same
//! `run_inline_automation_with_render_context` the handler used to spawn. What
//! changed is who does it and what they know:
//!
//! * **Identity is rebuilt, not inherited.** An executor is handed an
//!   assignment and nothing else — it never sees the platform context the
//!   driver loop built for the workspace, which carries no subject. The context
//!   comes from [`prepare`], through `custom_apps_gates::build_caller_context`
//!   with the caller's id and staging pin from the payload. See
//!   [`super::task`] for why a subject-less context would be an escalation,
//!   not a downgrade.
//! * **A run executes at most once.** The inline runner keeps no checkpoint,
//!   so a second attempt would start from the first step and repeat every
//!   side effect the first one finished (`http_request`, a write, an email).
//!   An attempt therefore takes the run before it runs anything — one atomic
//!   stamp of `execution_started_at` (`settle::begin_execution`) — and an
//!   attempt that finds the stamp already there never runs it. Whether it
//!   closes the run depends on the other attempt's heartbeat
//!   ([`heartbeat`]): stale, and the run is closed `failed` with
//!   [`settle::INTERRUPTED_CODE`] for the user to start again (the bundle's
//!   poll already handles `failed`); fresh, and that attempt is still
//!   executing, so this one steps aside and writes nothing
//!   (`admission::on_claim`). What the queue adds is that a dead driver's
//!   run is closed within a couple of minutes, rather than sitting `running`
//!   for two hours.
//! * **The row closes before the outcome is reported**, and only ever from
//!   `running` ([`super::settle`]). A second attempt that finds the row
//!   already closed does nothing.

use std::collections::HashMap;
use std::sync::Arc;

use agentic_core::delegation::{TaskAssignment, TaskOutcome};
use agentic_pipeline::automation_run::AutomationRunError;
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use entity::customer_app_procedure_runs as proc_run;
use entity::prelude::Workspaces;
use sea_orm::{DatabaseConnection, EntityTrait};
use sentry::SentryFutureExt;
use serde_json::Value as JsonValue;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;
use uuid::Uuid;

use super::settle::{self, Guard};
use super::task::ProcedureRunTask;
use crate::agentic_wiring::OxyProjectContext;
use admission::{admit, begin};

mod admission;
mod heartbeat;

/// The event a run writes to its own log once its context exists, naming the
/// identity it is about to act as — read from the built context, not from the
/// payload, so it records what the run *has* rather than what it was sent.
pub const STARTED_EVENT: &str = "procedure_run_started";

pub struct ProcedureRunExecutor {
    pub db: DatabaseConnection,
}

#[async_trait]
impl TaskExecutor for ProcedureRunExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let task = ProcedureRunTask::from_spec(&assignment.spec)?;

        let span = tracing::info_span!(
            "custom_app_procedure.job",
            run_id = %task.run_id,
            workspace_id = %task.workspace_id,
            procedure = %task.procedure_id,
        );
        let (event_tx, event_rx) = mpsc::channel(16);
        let (outcome_tx, outcome_rx) = mpsc::channel(4);
        let cancel = CancellationToken::new();
        let job = Job {
            db: self.db.clone(),
            task,
            // The worker trips the token it gets back on `ExecutingTask`; this
            // clone is the end that watches it.
            cancel: cancel.clone(),
            event_tx,
        };
        // A job has no request hub. The automation is tenant-authored and its
        // failures carry its SQL and results, so give it a hub tagged as a
        // custom-app surface (`middlewares::sentry_surface`).
        tokio::spawn(
            async move {
                let outcome = run_job(job).await;
                let _ = outcome_tx.send(outcome).await;
            }
            .instrument(span)
            .bind_hub(crate::server::api::middlewares::sentry_surface::custom_app_hub()),
        );

        Ok(ExecutingTask {
            events: event_rx,
            outcomes: outcome_rx,
            cancel,
            answers: None,
        })
    }
}

struct Job {
    db: DatabaseConnection,
    task: ProcedureRunTask,
    cancel: CancellationToken,
    event_tx: mpsc::Sender<(String, JsonValue)>,
}

/// Everything a run needs once its identity has been re-established.
pub struct PreparedRun {
    /// Built for the caller: their id as subject, no role, at their pin.
    pub context: OxyProjectContext,
    pub automation: agentic_automation::AutomationConfig,
    pub render_context: Option<JsonValue>,
}

/// Rebuild, on the driver, what the handler had in hand when it queued the run.
///
/// The workspace row is re-read rather than carried: it is what names the
/// working copy and the promoted revision *now*, on *this* node.
pub async fn prepare(
    db: &DatabaseConnection,
    task: &ProcedureRunTask,
) -> Result<PreparedRun, String> {
    let automation = task.automation_config()?;
    let workspace = Workspaces::find_by_id(task.workspace_id)
        .one(db)
        .await
        .map_err(|e| format!("workspace lookup failed: {e}"))?
        .ok_or_else(|| "the run's workspace no longer exists".to_string())?;
    let context = crate::server::api::custom_apps_gates::build_caller_context(
        &workspace,
        task.user_id,
        task.workspace_id,
        task.staging_pin,
    )
    .await
    .map_err(|response| {
        format!(
            "could not build the workspace context (status {})",
            response.status()
        )
    })?;
    Ok(PreparedRun {
        context,
        automation,
        render_context: task.render_context(),
    })
}

async fn run_job(job: Job) -> TaskOutcome {
    let Job {
        db,
        task,
        cancel,
        event_tx,
    } = job;
    let run_id = task.run_id;

    if let Err(outcome) = admit(&db, run_id).await {
        return outcome;
    }

    let prepared = match prepare(&db, &task).await {
        Ok(prepared) => prepared,
        Err(message) => {
            let e = AutomationRunError::Inline(message);
            return close_failed(&db, run_id, &e).await;
        }
    };

    // From here a step may run, so from here a later attempt must not.
    if let Err(outcome) = begin(&db, run_id).await {
        return outcome;
    }
    let _ = event_tx.send(started_event(&task, &prepared.context)).await;

    let workspace: Arc<dyn agentic_automation::WorkspaceContext> = Arc::new(prepared.context);
    let steps = agentic_pipeline::automation_run::run_inline_automation_with_render_context(
        workspace.as_ref(),
        prepared.automation,
        None,
        prepared.render_context,
        None,
    );
    execute_begun(&db, run_id, &cancel, heartbeat::INTERVAL, steps).await
}

/// Run a begun run's `steps` to a settled row, with its heartbeat beating
/// beside them from the stamp until the row is closed.
///
/// Takes the steps as a future and the cadence as a value so a test can hold
/// a run open and watch it beat; production passes the inline runner and
/// [`heartbeat::INTERVAL`].
async fn execute_begun(
    db: &DatabaseConnection,
    run_id: Uuid,
    cancel: &CancellationToken,
    beat_every: std::time::Duration,
    steps: impl Future<Output = Result<HashMap<String, JsonValue>, AutomationRunError>>,
) -> TaskOutcome {
    let beating = heartbeat::spawn(db.clone(), run_id, cancel.clone(), beat_every);
    let result = tokio::select! {
        // A trip of the token is a cancel (`cancel_subtree`), never a shutdown:
        // a dying process just stops, its claim is released or reaped, and the
        // attempt that next claims it finds the stamp beside a heartbeat that
        // has gone quiet and closes the run.
        _ = cancel.cancelled() => None,
        result = steps => Some(result),
    };

    let outcome = match result {
        None => close_cancelled(db, run_id).await,
        Some(Ok(outputs)) => close_done(db, run_id, &outputs).await,
        Some(Err(e)) => close_failed(db, run_id, &e).await,
    };
    // After the close, not before it: the run is never `running` and silent.
    beating.stop().await;
    outcome
}

/// [`STARTED_EVENT`], from the context the run is about to execute with.
fn started_event(task: &ProcedureRunTask, context: &OxyProjectContext) -> (String, JsonValue) {
    (
        STARTED_EVENT.to_string(),
        serde_json::json!({
            "procedure_id": task.procedure_id,
            "subject": context.subject(),
            "role": context.role().map(|r| r.as_str()),
            "staging_pin": task.staging_pin,
        }),
    )
}

async fn close_done(
    db: &DatabaseConnection,
    run_id: Uuid,
    outputs: &HashMap<String, JsonValue>,
) -> TaskOutcome {
    let terminal = settle::done(outputs);
    match settle::close_running(db, run_id, terminal, Guard::RunningAndNotCancelled).await {
        Ok(true) => TaskOutcome::Done {
            answer: settle::done_summary(outputs),
            metadata: None,
        },
        // Cancelled while it ran, or closed by someone else: the cancel wins.
        Ok(false) => close_cancelled(db, run_id).await,
        Err(e) => {
            tracing::error!(%run_id, error = %e, "automation run completion update failed");
            TaskOutcome::Failed(format!("record automation result: {e}"))
        }
    }
}

async fn close_failed(
    db: &DatabaseConnection,
    run_id: Uuid,
    e: &AutomationRunError,
) -> TaskOutcome {
    let message = e.to_string();
    match settle::close_running(db, run_id, settle::failed(e), Guard::RunningAndNotCancelled).await
    {
        Ok(true) => TaskOutcome::Failed(message),
        Ok(false) => close_cancelled(db, run_id).await,
        Err(db_err) => {
            tracing::error!(%run_id, error = %db_err, "automation run completion update failed");
            TaskOutcome::Failed(message)
        }
    }
}

/// Close a run an earlier attempt began and never finished, without running
/// it. `Failed` on the queue side too, with the same sentence, so the run's
/// driver-side record and its bundle-facing row say the same thing.
async fn close_interrupted(db: &DatabaseConnection, run_id: Uuid) -> TaskOutcome {
    let interrupted = TaskOutcome::Failed(settle::INTERRUPTED_MESSAGE.to_string());
    match settle::close_running(
        db,
        run_id,
        settle::interrupted(),
        Guard::RunningAndNotCancelled,
    )
    .await
    {
        Ok(true) => interrupted,
        // A cancel landed since the read: the user's stop is the truer close.
        Ok(false) => close_cancelled(db, run_id).await,
        Err(e) => {
            tracing::error!(%run_id, error = %e, "automation run interrupted update failed");
            interrupted
        }
    }
}

/// Close the run as cancelled if it is still open, and report what it ended
/// as. A row someone else already closed as `done` or `failed` stays that way.
async fn close_cancelled(db: &DatabaseConnection, run_id: Uuid) -> TaskOutcome {
    if let Err(e) = settle::close_running(db, run_id, settle::cancelled(), Guard::Running).await {
        tracing::error!(%run_id, error = %e, "automation run cancel update failed");
    }
    match proc_run::Entity::find_by_id(run_id).one(db).await {
        Ok(Some(row)) if row.status == "cancelled" => TaskOutcome::Cancelled,
        Ok(Some(_)) => TaskOutcome::Done {
            answer: "run was already closed".to_string(),
            metadata: None,
        },
        _ => TaskOutcome::Cancelled,
    }
}

#[cfg(test)]
mod tests;
