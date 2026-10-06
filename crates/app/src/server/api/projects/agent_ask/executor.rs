//! `AgentAskExecutor`: drives a queued custom-app ask
//! (`TaskSpec::Custom { kind: "custom_app_agent_ask" }`) on whichever process
//! claimed it. Registered into the `CustomTaskRegistry` by
//! `server::router::recovery`; `PipelineTaskExecutor` delegates the kind here.
//!
//! **Registered, and dormant: nothing enqueues the kind yet.** See
//! [`super::task`].
//!
//! The pipeline is the handler's own — [`super::pipeline::ask_pipeline`] is
//! the one definition both build from, so a queued ask carries its thread and
//! can stop to ask its user a question exactly as a handler-driven one does.
//! (`TaskSpec::Agent`, what the scheduler seeds, does neither: it never sets
//! the thread and it installs `NoClarificationProvider`.) What differs is who
//! runs it and what they know:
//!
//! * **Identity is rebuilt, not inherited.** An executor is handed an
//!   assignment and nothing else — never the platform context the driver loop
//!   built for the workspace, which carries no subject. [`prepare`] reads the
//!   caller off the run row and builds the context through
//!   `custom_apps_gates::build_caller_context`, the function the handler's own
//!   context goes through. A run with no caller record is failed, not driven.
//! * **An ask's pipeline starts at most once.** An analytics run keeps no
//!   checkpoint until it suspends, so a second attempt would start it again
//!   from the top: a second copy of every `text_delta` on a stream the client
//!   is reading, a second LLM bill, and every automation the agent delegates
//!   to run twice. An attempt therefore takes the run before it starts
//!   anything — one atomic stamp of `execution_started_at` — and an attempt
//!   that finds the stamp there never starts it ([`admission`]).
//! * **An attempt that does not start the pipeline reports through the same
//!   channels one that does would.** The coordinator owns the run's row and
//!   its event log, so a refusal is an event and an outcome ([`Settled`]),
//!   never a write of this executor's own.

use std::sync::Arc;

use agentic_core::delegation::{TaskAssignment, TaskOutcome};
use agentic_pipeline::AnalyticsSchemaCatalog;
use agentic_runtime::entity::run;
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use sentry::SentryFutureExt;
use serde_json::Value as JsonValue;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use super::caller::CallerContextError;
use super::pipeline::ask_pipeline;
use super::task::{self, QueuedAsk};
use crate::agentic_wiring::OxyProjectContext;

mod admission;
mod heartbeat;

pub use admission::INTERRUPTED_MESSAGE;

/// The schema cache a driver shares across the runs it drives.
pub type SchemaCache =
    Arc<std::sync::Mutex<std::collections::HashMap<String, AnalyticsSchemaCatalog>>>;

pub struct AgentAskExecutor {
    pub db: DatabaseConnection,
    /// The driver's shared schema cache, as the handler passes its process's.
    pub schema_cache: Option<SchemaCache>,
}

#[async_trait]
impl TaskExecutor for AgentAskExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        task::accept(&assignment.spec)?;
        let span = tracing::info_span!("custom_app_agent_ask.job", run_id = %assignment.run_id);
        // A driver has no request hub. The ask carries a bundle's prompt and
        // the warehouse errors answering it, and the pipeline's own tasks
        // inherit the hub that is current when they are spawned — so the
        // custom-app tag has to be in place before the pipeline starts
        // (`middlewares::sentry_surface`).
        let hub = crate::server::api::middlewares::sentry_surface::custom_app_hub();
        Ok(self
            .claim(&assignment.run_id)
            .instrument(span)
            .bind_hub(hub)
            .await)
    }
}

impl AgentAskExecutor {
    /// One claimed attempt, start to report: the pipeline's own task when this
    /// attempt is the one that starts it, a settled one otherwise.
    async fn claim(&self, run_id: &str) -> ExecutingTask {
        let run = match admission::admit(&self.db, run_id).await {
            Ok(run) => run,
            Err(settled) => return settled.into_task(),
        };
        let prepared = match prepare(&self.db, &run).await {
            Ok(prepared) => prepared,
            Err(e) => return unprepared(run_id, e).into_task(),
        };
        // From here the pipeline may run, so from here a later attempt must not.
        if let Err(settled) = admission::begin(&self.db, run_id).await {
            return settled.into_task();
        }
        let beating = heartbeat::spawn(self.db.clone(), run_id.to_string(), heartbeat::INTERVAL);
        self.start(prepared, beating).await
    }

    /// Start the pipeline of a begun ask. Its heartbeat beats beside it until
    /// the pipeline stops — at a terminal outcome or at a suspension.
    async fn start(&self, prepared: PreparedAsk, beating: heartbeat::Beating) -> ExecutingTask {
        let PreparedAsk { ask, context } = prepared;
        let builder = ask_pipeline(
            Arc::new(context),
            ask.workspace_id,
            &ask.question,
            ask.thread_id,
            self.schema_cache.clone(),
            &ask.agent_id,
        )
        .existing_run(ask.run_id.clone());
        // Boxed: the start's future is a large one, and this runs on a driver
        // task's stack beside the claim loop's.
        match Box::pin(builder.start(&self.db)).await {
            Ok(started) => {
                let (task, pipeline_stopped) = started.into_executing_task();
                tokio::spawn(async move {
                    let _ = pipeline_stopped.await;
                    beating.stop().await;
                });
                task
            }
            Err(e) => {
                beating.stop().await;
                tracing::warn!(run_id = %ask.run_id, error = %e, "agent ask: pipeline start failed");
                Settled::failed(e.to_string()).into_task()
            }
        }
    }
}

/// Everything an ask needs once its identity has been re-established.
pub struct PreparedAsk {
    pub ask: QueuedAsk,
    /// Built for the caller: their id as subject, no role, at their pin.
    pub context: OxyProjectContext,
}

/// Why a claimed ask could not be prepared.
#[derive(Debug, thiserror::Error)]
pub enum PrepareError {
    /// The run row does not describe an ask anyone may drive: it names no
    /// agent, or records no caller (or one that does not read back).
    #[error("{0}")]
    NotAnAsk(String),
    /// The caller's context could not be built.
    #[error(transparent)]
    Context(#[from] CallerContextError),
}

impl PrepareError {
    /// Might the same attempt succeed a minute from now?
    ///
    /// Yes for a failed read of the workspace row, and for a context that
    /// answers `503` — the builder's "not compiled yet, retry". Nothing has
    /// started at this point, so trying again costs nothing but the wait.
    ///
    /// No for everything else, including a `500` from the context builder: it
    /// cannot tell a blip from a workspace whose config will never load, and
    /// retrying the second would leave the bundle's stream open for a day
    /// where an `error` now lets its user ask again. That is also what the
    /// same failure does to a handler-driven ask today.
    fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::Context(CallerContextError::Lookup(_))
                | Self::Context(CallerContextError::Build(503))
        )
    }
}

/// Rebuild, on the driver, what the handler had in hand when it started the
/// ask. The workspace row is re-read rather than carried: it is what names the
/// working copy and the promoted revision *now*, on *this* node.
pub async fn prepare(
    db: &DatabaseConnection,
    run: &run::Model,
) -> Result<PreparedAsk, PrepareError> {
    let ask = QueuedAsk::from_run(run).map_err(PrepareError::NotAnAsk)?;
    let context = ask.caller.context(db, ask.workspace_id).await?;
    Ok(PreparedAsk { ask, context })
}

/// The report of an attempt whose ask could not be prepared. The run is not
/// begun yet, so a failure that may pass is handed back to be tried again;
/// one that will not is the end of the ask.
fn unprepared(run_id: &str, e: PrepareError) -> Settled {
    if e.is_transient() {
        admission::step_aside(run_id, format!("the ask could not be prepared yet: {e}"))
    } else {
        Settled::failed(e.to_string())
    }
}

/// The report of an attempt that does not start the pipeline: at most one
/// event for the run's log, then one outcome.
#[derive(Debug)]
pub(super) struct Settled {
    event: Option<(String, JsonValue)>,
    outcome: TaskOutcome,
}

impl Settled {
    /// An outcome and nothing for the log.
    fn only(outcome: TaskOutcome) -> Self {
        Self {
            event: None,
            outcome,
        }
    }

    /// The ask ends here, failed, without its pipeline having run. The bundle
    /// reads the run's event log and never its row, so the failure has to be
    /// on the log too: an `error` event is what ends its stream.
    fn failed(message: String) -> Self {
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
    fn into_task(self) -> ExecutingTask {
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

#[cfg(test)]
mod tests;
