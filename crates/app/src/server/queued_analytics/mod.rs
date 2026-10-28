//! An analytics run started from the queue: what every executor of one shares.
//!
//! Two kinds of run are seeded by a request and started by whichever driver
//! claims them (`agentic_pipeline::queued_run`): a custom-app ask
//! (`projects::agent_ask::executor`) and a chat run ([`chat`]). They differ in
//! who the run is for and what its pipeline is built with. They share the rule
//! that makes a claim safe to hand on, and that rule lives here:
//!
//! **An analytics run's pipeline starts at most once.** A run keeps no
//! checkpoint until it suspends, so a second attempt would start it again from
//! the top: a second copy of every `text_delta` on a stream the client is
//! reading, a second LLM bill, and every automation the agent delegates to run
//! twice. So a claimed attempt reads the run ([`admission::admit`]), builds
//! what it needs, takes the run with one atomic stamp
//! ([`admission::begin`]), and only then starts the pipeline, with a
//! heartbeat of its own beside it ([`heartbeat`]). A run that is already
//! begun, parked at a suspension, cancelled or ended is settled instead —
//! **resume from a suspension checkpoint, otherwise close as interrupted,
//! never re-run** — and the settlement is reported through the channels a
//! running pipeline would use ([`Settled`]), since the coordinator owns the
//! run's row and its event log. [`attempt`] is that sequence, once.

use std::future::Future;

use agentic_core::delegation::TaskOutcome;
use agentic_runtime::entity::run;
use agentic_runtime::worker::ExecutingTask;
use sea_orm::DatabaseConnection;
use serde_json::Value as JsonValue;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub mod admission;
pub mod chat;
pub(crate) mod heartbeat;
pub mod requester;

pub use admission::INTERRUPTED_MESSAGE;

/// The feature flag that puts a plain chat start and a requester's
/// agentic-workflow start on the queue (`AgenticState::queued_starts`).
/// Registered in `feature_flags::registry`, default off.
///
/// Off, both are driven in the request's process, as they were before the
/// queued chat existed. It is a switch rather than a release order because no
/// driver checks which run kinds it can execute before it claims one
/// (`find_pending_global_runs` selects every queued Global run; an unknown
/// `TaskSpec::Custom` kind fails in `PipelineTaskExecutor`), so in a rolling
/// deploy a pod from before the chat executor would claim a queued chat and fail
/// it. Turn it on once every pod runs a build that registers `ChatRunExecutor`.
pub const QUEUED_STARTS_FLAG: &str = "queued_chat_and_workflow_starts";

/// Whether [`QUEUED_STARTS_FLAG`] is on, read per start.
pub fn queued_starts_enabled() -> bool {
    crate::server::feature_flags::is_enabled(QUEUED_STARTS_FLAG)
}

/// One claimed attempt, start to report.
///
/// `prepare` turns the admitted run into whatever the pipeline is built from —
/// the run's identity, re-established — or into this attempt's final report.
/// `start` gets that and the heartbeat already beating, and must stop the
/// heartbeat when the pipeline stops (or fails to start).
///
/// The stamp is taken between the two: after the context is built, so a
/// driver that dies while building it leaves a run nothing has started, which
/// the next attempt may run; immediately before the start, so nothing between
/// the stamp and the pipeline can fail without being the pipeline's failure.
pub(crate) async fn attempt<P, Prep, PrepFut, Start, StartFut>(
    db: &DatabaseConnection,
    run_id: &str,
    prepare: Prep,
    start: Start,
) -> ExecutingTask
where
    Prep: FnOnce(run::Model) -> PrepFut,
    PrepFut: Future<Output = Result<P, Settled>>,
    Start: FnOnce(P, heartbeat::Beating) -> StartFut,
    StartFut: Future<Output = ExecutingTask>,
{
    let run = match admission::admit(db, run_id).await {
        Ok(run) => run,
        Err(settled) => return settled.into_task(),
    };
    let prepared = match prepare(run).await {
        Ok(prepared) => prepared,
        Err(settled) => return settled.into_task(),
    };
    if let Err(settled) = admission::begin(db, run_id).await {
        return settled.into_task();
    }
    let beating = heartbeat::spawn(db.clone(), run_id.to_string(), heartbeat::INTERVAL);
    start(prepared, beating).await
}

/// Hand a started pipeline's task to the worker, and stop `beating` when the
/// pipeline stops — at a terminal outcome or at a suspension.
pub(crate) fn beside_the_pipeline(
    started: agentic_pipeline::StartedPipeline,
    beating: heartbeat::Beating,
) -> ExecutingTask {
    let (task, pipeline_stopped) = started.into_executing_task();
    tokio::spawn(async move {
        let _ = pipeline_stopped.await;
        beating.stop().await;
    });
    task
}

/// The report of an attempt that does not start the pipeline: at most one
/// event for the run's log, then one outcome.
#[derive(Debug)]
pub struct Settled {
    pub(crate) event: Option<(String, JsonValue)>,
    pub(crate) outcome: TaskOutcome,
}

impl Settled {
    /// An outcome and nothing for the log.
    pub(crate) fn only(outcome: TaskOutcome) -> Self {
        Self {
            event: None,
            outcome,
        }
    }

    /// The run ends here, failed, without its pipeline having run. A client
    /// reads the run's event log, so the failure has to be on the log too: an
    /// `error` event is what ends its stream.
    pub(crate) fn failed(message: String) -> Self {
        Self {
            event: Some((
                "error".to_string(),
                serde_json::json!({ "message": message, "trace_id": "" }),
            )),
            outcome: TaskOutcome::Failed(message),
        }
    }

    /// As the task a worker forwards: the event, then the outcome, then both
    /// channels closed.
    pub(crate) fn into_task(self) -> ExecutingTask {
        let (event_tx, events) = mpsc::channel(1);
        let (outcome_tx, outcomes) = mpsc::channel(1);
        if let Some(event) = self.event {
            // Capacity one, one send: cannot be full.
            let _ = event_tx.try_send(event);
        }
        let _ = outcome_tx.try_send(self.outcome);
        ExecutingTask {
            events,
            outcomes,
            cancel: CancellationToken::new(),
            answers: None,
        }
    }
}

/// A `running` analytics root carrying `metadata`, for tests that read a run
/// row without a database.
#[cfg(test)]
pub(crate) fn run_for_test(metadata: Option<serde_json::Value>) -> run::Model {
    let now = agentic_runtime::crud::now();
    run::Model {
        id: "run-1".into(),
        question: "how did the stores do?".into(),
        answer: None,
        error_message: None,
        thread_id: Some(uuid::Uuid::new_v4()),
        source_type: Some("analytics".into()),
        metadata,
        parent_run_id: None,
        schedule_id: None,
        task_status: Some("running".into()),
        task_metadata: None,
        attempt: 0,
        recovery_requested_at: None,
        driver_id: None,
        driver_heartbeat_at: None,
        cancel_requested_at: None,
        workspace_id: uuid::Uuid::new_v4(),
        created_at: now,
        updated_at: now,
    }
}

#[cfg(test)]
mod tests;
