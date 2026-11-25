//! Analytics on the react runtime, served: a chat run on an org where the
//! `react_runtime` flag is on, the only switch (`pick::picks_served`).
//!
//! The run is built inside `start_analytics` (the context root it reads dies
//! when the start returns) and the loop runs on its own task behind a
//! [`RowHandle`]. `into_executing_task` forwards its rows as they are, so the
//! coordinator persists exactly what the runtime wrote.
//!
//! Row ownership matches the FSM's: the coordinator writes only the run's
//! status and suspension, never an event. The runtime writes the one `done`
//! or `error` of a finished loop; this module writes the rows the loop
//! cannot: an `error` when the build fails or a start is refused, and the
//! FSM's `awaiting_input` on a suspend (which writes no terminal row).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use agentic_analytics::AnalyticsEvent;
use agentic_analytics::config::{AgentConfig, BuildContext, ResolvedModelInfo};
use agentic_connector::DatabaseConnector;
use agentic_core::delegation::SuspendReason;
use agentic_core::events::{CoreEvent, Event, HumanInputQuestion};
use agentic_core::hub_task::spawn_with_hub;
use agentic_core::react::{EventSink, PriorTurn};
use agentic_runtime::handle::PipelineOutcome;
use agentic_runtime::react::{LoopOutcome, SuspensionInput, suspension_data};
use sea_orm::DatabaseConnection;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::platform::PlatformContext;
use crate::react_run::{ReactRun, build_react_run, run_react};
use crate::{ErasedHandle, StartedPipeline, ThinkingMode};

/// The run metadata key a recovery with no suspension routes on.
pub(crate) const RUNTIME_KEY: &str = "runtime";
pub(crate) const REACT: &str = "react";
pub(crate) const FSM: &str = "fsm";

/// A served react run: its rows, its outcome, and the task running it.
pub(crate) struct RowHandle {
    pub rows: mpsc::Receiver<(String, Value)>,
    pub outcomes: mpsc::Receiver<PipelineOutcome>,
    pub cancel: CancellationToken,
    pub join: tokio::task::JoinHandle<()>,
}

#[path = "react_memory_served.rs"]
mod memory;
#[path = "react_served_pick.rs"]
mod pick;
pub(crate) use pick::{PickInput, ServeCheck, decide, serves_react, stamp_runtime};
#[cfg(test)]
pub(crate) use pick::{cannot_serve, picks_served, runtime_patch};

/// What `start_analytics` resolved for the FSM, handed to the react build.
#[derive(Clone)]
pub(crate) struct ServedInputs {
    pub agent_id: String,
    pub workspace_id: uuid::Uuid,
    pub connectors: HashMap<String, Arc<dyn DatabaseConnector>>,
    /// The agent's databases in order; the first one built is the default.
    pub databases: Vec<String>,
    pub project_model: Option<ResolvedModelInfo>,
    pub domain_docs: Option<Vec<String>>,
    /// The user's extended-thinking toggle, applied as the FSM applies it.
    pub thinking_mode: ThinkingMode,
}

/// The question a served run answers, and where.
pub(crate) struct ServedRun<'a> {
    pub run_id: &'a str,
    pub thread_id: Option<uuid::Uuid>,
    pub question: String,
}

/// Builds the react run and starts it. A failed build is not a start error:
/// the run row exists, so the handle writes its `error` row and reports
/// `Failed`, as a loop failure would. `log` is the run's log as
/// `start_analytics` read it ([`crate::react_resume::prior_log`]): one that
/// holds an attempt is continued, and one that could not be read refuses
/// the start (one `error` row) rather than start over on top of it.
pub(crate) async fn start(
    platform: &dyn PlatformContext,
    db: &DatabaseConnection,
    config: &AgentConfig,
    inputs: ServedInputs,
    base_dir: &Path,
    run: ServedRun<'_>,
    log: crate::react_resume::PriorLog,
) -> StartedPipeline {
    let prior = match run.thread_id {
        Some(tid) => prior_turns(db, tid, run.run_id).await,
        None => Vec::new(),
    };
    let (run_id, question) = (run.run_id.to_string(), run.question);
    let handle = match log {
        Err(e) => {
            let refused = Err(format!("cannot read the run's event log: {e}"));
            serve(refused, run_id.clone(), prior, question)
        }
        Ok(log) => {
            let built = build_served(platform, db, config, inputs, base_dir).await;
            let built = with_archive(platform, built, &run_id);
            if crate::react_resume::continues(&log) {
                // A re-driven start (a released or reaped claim): the log
                // already holds an attempt, so it is continued.
                tracing::info!(run_id, "react start finds an earlier attempt; resuming it");
                let resumed = crate::react_resume::Resumed {
                    run_id: run_id.clone(),
                    question,
                    prior,
                };
                crate::react_resume::serve_resumed(built, db, resumed, &log, None)
            } else {
                serve(built, run_id.clone(), prior, question)
            }
        }
    };
    StartedPipeline {
        run_id,
        source_type: "analytics".to_string(),
        inner: ErasedHandle::React(handle),
        starting_driver: None,
    }
}

/// `inputs` with each connector that has no read-only path replaced by the
/// platform's read-only one for that database, where it has one
/// ([`PlatformContext::read_only_connector`]: an `airhouse_managed` database
/// over a credential Airhouse confirmed read-only). React queries only
/// through these; the FSM keeps the originals. Every react entry runs it: the
/// start's check ([`pick::cannot_serve`]) and the start and resume builds
/// ([`build_served`]).
pub(crate) async fn read_only_inputs(
    platform: &dyn PlatformContext,
    mut inputs: ServedInputs,
) -> ServedInputs {
    let mut names: Vec<String> = inputs.connectors.keys().cloned().collect();
    names.sort_unstable();
    for name in names {
        if inputs.connectors[&name].supports_read_only() {
            continue;
        }
        if let Some(read_only) = platform.read_only_connector(&name).await {
            inputs.connectors.insert(name, read_only);
        }
    }
    inputs
}

/// `built` storing its query results in the platform's archive, when it has
/// one, under the platform's workspace and the run's id. A start and a resume
/// both attach it, so a resumed run reads back what an earlier attempt
/// stored. The workspace is `PlatformContext::workspace_id`, the one the
/// results route reads with, never the inputs' (a resume's can be unset).
pub(crate) fn with_archive(
    platform: &dyn PlatformContext,
    built: Result<ReactRun, String>,
    run_id: &str,
) -> Result<ReactRun, String> {
    if let (Ok(run), Some(archive)) = (&built, platform.result_archive()) {
        run.analytics
            .results
            .attach_archive(archive, platform.workspace_id(), run_id);
    }
    built
}

/// The react run a served start or resume runs, with the requesting user's
/// memory ([`memory::served_memory`]).
pub(crate) async fn build_served(
    platform: &dyn PlatformContext,
    db: &DatabaseConnection,
    config: &AgentConfig,
    inputs: ServedInputs,
    base_dir: &Path,
) -> Result<ReactRun, String> {
    let inputs = read_only_inputs(platform, inputs).await;
    let (workspace_id, agent_id) = (inputs.workspace_id, inputs.agent_id.clone());
    let ctx = build_context(platform, config, inputs);
    let built = build_react_run(platform, config, ctx, base_dir).await?;
    let memory = memory::served_memory(platform, db, workspace_id).await;
    let owner = (memory.writes.is_some())
        .then(|| platform.requester())
        .flatten()
        .map(|r| agentic_runtime::react::NoteOwner {
            workspace_id,
            user_id: r.user_id,
        });
    let mut built = built.with_memory(memory);
    built.after_done = owner.map(|owner| crate::queued_run::memory_merge::MergeTrigger {
        db: db.clone(),
        owner,
        agent_id,
    });
    Ok(built)
}

/// The FSM solver's build context, from the same resolved inputs (the
/// headless rule for the default: the first declared database that built),
/// with the same extended-thinking overrides `start_pipeline` applies.
pub(crate) fn build_context(
    platform: &dyn PlatformContext,
    config: &AgentConfig,
    inputs: ServedInputs,
) -> BuildContext {
    let default = inputs
        .databases
        .iter()
        .find(|name| inputs.connectors.contains_key(*name))
        .cloned();
    let (thinking_override, model_override) =
        config.extended_thinking_overrides(inputs.thinking_mode.is_extended());
    BuildContext {
        extra_connectors: inputs.connectors,
        extra_default_connector: default,
        project_model_info: inputs.project_model,
        genai: agentic_llm::GenAiContext {
            agent_name: Some(inputs.agent_id),
            workspace_id: Some(inputs.workspace_id.to_string()),
            ..Default::default()
        },
        thinking_override,
        model_override,
        timezone: platform.timezone(),
        domain_docs: inputs.domain_docs,
        ..BuildContext::default()
    }
}

/// The thread's finished turns from before this run started (up to ten,
/// read as the FSM reads them): only runs created before `run_id`, so a turn
/// that finished while this run was parked never becomes its prior turn on
/// a resume. A read that fails is logged and gives no prior turns.
pub(crate) async fn prior_turns(
    db: &DatabaseConnection,
    thread_id: uuid::Uuid,
    run_id: &str,
) -> Vec<PriorTurn> {
    let started = match agentic_runtime::crud::get_run(db, run_id).await {
        Ok(Some(run)) => run.created_at,
        Ok(None) => {
            tracing::warn!(run_id, "react: run row missing; no prior turns");
            return Vec::new();
        }
        Err(e) => {
            tracing::warn!(run_id, error = %e, "react: run row unreadable; no prior turns");
            return Vec::new();
        }
    };
    let history =
        agentic_runtime::crud::get_thread_history_before(db, thread_id, started, 10).await;
    let turns = history.unwrap_or_else(|e| {
        tracing::warn!(run_id, error = %e, "react: thread history unreadable; no prior turns");
        Vec::new()
    });
    turns
        .into_iter()
        .map(|t| PriorTurn {
            question: t.question,
            answer: t.answer,
            results_summary: String::new(),
        })
        .collect()
}

/// Runs `built` on its own task: the loop when it built, else one `error`
/// row. Every path sends exactly one outcome.
pub(crate) fn serve(
    built: Result<ReactRun, String>,
    run_id: String,
    prior: Vec<PriorTurn>,
    question: String,
) -> RowHandle {
    let (id, asked) = (run_id.clone(), question.clone());
    serve_with(
        built,
        run_id,
        question,
        move |built, ev, token| async move {
            match built {
                Ok(run) => run_react(run, id, prior, asked, ev, token).await,
                Err(reason) => LoopOutcome::Refused { reason },
            }
        },
    )
}

/// [`serve`] with what `drive` does with the build: a start runs
/// [`run_react`] or refuses; a resume continues the log, or on a failed
/// build returns the end the log already records. `question` is the run's
/// original question, which a suspend records.
pub(crate) fn serve_with<F, Fut>(
    built: Result<ReactRun, String>,
    run_id: String,
    question: String,
    drive: F,
) -> RowHandle
where
    F: FnOnce(Result<ReactRun, String>, EventSink, CancellationToken) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = LoopOutcome> + Send + 'static,
{
    let (rows_tx, rows) = mpsc::channel::<(String, Value)>(256);
    let (outcome_tx, outcomes) = mpsc::channel::<PipelineOutcome>(4);
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let after_done = built.as_ref().ok().and_then(|r| r.after_done.clone());
    let join = spawn_with_hub(async move {
        let outcome = drive(built, rows_tx.clone(), token).await;
        let done = matches!(outcome, LoopOutcome::Done { .. });
        let outcome = settle(&rows_tx, &run_id, &question, outcome).await;
        if outcome_tx.send(outcome).await.is_err() {
            tracing::warn!(run_id, "react outcome dropped: nobody is driving the run");
        }
        // After the outcome is sent, so the run never waits on it.
        if let Some(trigger) = after_done.filter(|_| done) {
            trigger.after_done(&run_id).await;
        }
    });
    RowHandle {
        rows,
        outcomes,
        cancel,
        join,
    }
}

/// Writes the rows the loop did not, then maps its outcome to the pipeline's.
/// `Refused` (a failed build, or a resume refused even without its answer)
/// wrote no row, so it gets the `error` row a `Failed` already has.
async fn settle(
    ev: &EventSink,
    run_id: &str,
    question: &str,
    outcome: LoopOutcome,
) -> PipelineOutcome {
    match outcome {
        LoopOutcome::Done { answer } => PipelineOutcome::Done {
            answer: answer.text,
            metadata: None,
        },
        LoopOutcome::Failed { message } => PipelineOutcome::Failed(message),
        LoopOutcome::Cancelled => PipelineOutcome::Cancelled,
        LoopOutcome::Refused { reason } => {
            let error = CoreEvent::Error {
                message: reason.clone(),
                trace_id: run_id.to_string(),
            };
            emit(ev, error).await;
            PipelineOutcome::Failed(reason)
        }
        LoopOutcome::Suspended {
            call_id,
            question: asked,
            suggestions,
        } => {
            let parked = Parked {
                run_id,
                call_id: &call_id,
                question: &asked,
                suggestions: &suggestions,
            };
            suspend(ev, parked, question).await
        }
    }
}

/// A run parked on a question.
pub(crate) struct Parked<'a> {
    pub run_id: &'a str,
    pub call_id: &'a str,
    pub question: &'a str,
    pub suggestions: &'a [String],
}

/// Emits the FSM's `awaiting_input` row and returns the suspend outcome whose
/// resume data names the parked call (`from_state: react`).
pub(crate) async fn suspend(ev: &EventSink, p: Parked<'_>, original: &str) -> PipelineOutcome {
    let questions = vec![HumanInputQuestion {
        prompt: p.question.to_string(),
        suggestions: p.suggestions.to_vec(),
    }];
    let waiting = CoreEvent::AwaitingHumanInput {
        questions: questions.clone(),
        from_state: REACT.into(),
        trace_id: p.run_id.to_string(),
    };
    emit(ev, waiting).await;
    let resume_data = suspension_data(SuspensionInput {
        run_id: p.run_id,
        call_id: p.call_id,
        question: p.question,
        suggestions: p.suggestions,
        original_input: original,
        trace_id: p.run_id,
    });
    PipelineOutcome::Suspended {
        reason: SuspendReason::HumanInput { questions },
        resume_data,
        trace_id: p.run_id.to_string(),
    }
}

async fn emit(ev: &EventSink, e: CoreEvent) {
    if ev
        .send(Event::<AnalyticsEvent>::Core(e).serialize())
        .await
        .is_err()
    {
        tracing::warn!("react: event sink closed; row dropped");
    }
}

/// The React arm of `into_executing_task`: rows forwarded as they are (the
/// runtime already dropped any reserved tag a tool tried to write), the
/// outcome through the FSM's own forwarder, cancellation passed down.
pub(crate) fn spawn_forwarders(
    handle: RowHandle,
    event_tx: mpsc::Sender<(String, Value)>,
    outcome_tx: mpsc::Sender<agentic_core::delegation::TaskOutcome>,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    let RowHandle {
        mut rows,
        outcomes,
        cancel: run_cancel,
        join,
    } = handle;
    spawn_with_hub(async move {
        cancel.cancelled().await;
        run_cancel.cancel();
    });
    let rows_task = spawn_with_hub(async move {
        while let Some(row) = rows.recv().await {
            if event_tx.send(row).await.is_err() {
                break;
            }
        }
    });
    let outcomes_task = spawn_with_hub(crate::forward_outcomes(outcomes, outcome_tx));
    spawn_with_hub(async move {
        let _ = tokio::join!(rows_task, outcomes_task, join);
    })
}

#[cfg(test)]
#[path = "react_served_tests.rs"]
mod tests;
