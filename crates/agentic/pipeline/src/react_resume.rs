//! A served react run, continued from its log: a person's answer to its
//! `ask_user`, a crash recovery, or a start re-driven onto a log that
//! already holds an attempt (a claim a deploy released or the reaper
//! requeued). The run's event log is the source of truth, so nothing in
//! memory survives from the start: the toolset and engine are rebuilt from
//! the agent's config (`build_react_run`, with a start's refusals) and the
//! transcript from the log (`resume_loop` over a [`DbTranscriptStore`]).
//!
//! What routes a resume here is only what the run persisted: a suspension
//! whose `from_state` is `react`, or, for a recovery with no suspension, the
//! run's `runtime: react` metadata. Never the agent's `runtime:` key or the
//! org's flag: a run started on react finishes on react.
//!
//! Rows follow the served start ([`crate::react_served`]): the loop writes the
//! `done`/`error` of a loop it finishes, a suspend writes `awaiting_input`,
//! and a failed rebuild or a refused resume gets one `error` row, unless the
//! log already ended: a run never gets a second terminal row.

use std::path::Path;

use agentic_analytics::config::AgentConfig;
use agentic_analytics::react::citations::strip_citations;
use agentic_core::human_input::SuspendedRunData;
use agentic_core::react::{EventSink, PriorTurn, Transcript};
use agentic_runtime::react::{
    DbTranscriptStore, LoopCx, LoopOutcome, ResumeInput, TranscriptStore, already_terminal,
    resume_loop,
};
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::platform::PlatformContext;
use crate::react_run::{ReactRun, build_react_run, loop_config};
use crate::react_served::{
    REACT, RUNTIME_KEY, RowHandle, ServedInputs, ServedRun, build_context, prior_turns, serve_with,
};
use crate::{ErasedHandle, StartedPipeline, ThinkingMode};

/// Whether `data` is a react run's suspension (or [`recovery_data`]'s
/// stand-in for one).
pub(crate) fn is_react(data: &SuspendedRunData) -> bool {
    data.from_state == REACT
}

/// The resume a run with no suspension gets when its metadata says its
/// latest attempt ran react: a react suspension naming no call, so the
/// resume is a crash recovery (`answer: None`). `None` for any other run,
/// which stays on the FSM's path.
pub(crate) fn recovery_data(run: &agentic_runtime::entity::run::Model) -> Option<SuspendedRunData> {
    let runtime = run.metadata.as_ref()?.get(RUNTIME_KEY)?.as_str()?;
    (runtime == REACT).then(|| SuspendedRunData {
        from_state: REACT.into(),
        original_input: run.question.clone(),
        trace_id: run.id.clone(),
        stage_data: json!({ "run_id": run.id }),
        question: String::new(),
        suggestions: Vec::new(),
    })
}

/// The runtime's resume input: a blank answer (crash recovery passes `""`)
/// is no answer, and the call id is the one the suspension recorded.
pub(crate) fn resume_input(run_id: &str, data: &SuspendedRunData, answer: &str) -> ResumeInput {
    let call_id = data
        .stage_data
        .get("call_id")
        .and_then(Value::as_str)
        .map(String::from);
    ResumeInput {
        run_id: run_id.to_string(),
        answer: Some(answer.to_string()).filter(|a| !a.trim().is_empty()),
        call_id,
    }
}

/// The thinking mode `run_id` started with, from its metadata (parsed as
/// `ChatStart::from_run` parses it). A resume rebuilds on the model the
/// start ran, so the turns it recorded, thinking blocks included, replay into
/// the model that wrote them. An unreadable row resumes at `Auto`.
pub(crate) async fn started_thinking_mode(db: &DatabaseConnection, run_id: &str) -> ThinkingMode {
    match agentic_runtime::crud::get_run(db, run_id).await {
        Ok(run) => recorded_thinking_mode(run.and_then(|r| r.metadata).as_ref()),
        Err(e) => {
            tracing::warn!(run_id, error = %e, "react resume: run row unreadable; thinking auto");
            ThinkingMode::Auto
        }
    }
}

/// `metadata.thinking_mode`: `extended_thinking`, else `Auto`.
pub(crate) fn recorded_thinking_mode(metadata: Option<&Value>) -> ThinkingMode {
    match metadata.and_then(|m| m.get("thinking_mode")?.as_str()) {
        Some("extended_thinking") => ThinkingMode::ExtendedThinking,
        _ => ThinkingMode::Auto,
    }
}

/// Rebuilds the react run and resumes it from its log. A failed rebuild is
/// not a resume error: the run row exists, so the handle writes its one
/// `error` row and reports `Failed`, as a failed start does, unless the log
/// already ended (see [`serve_resumed`]).
pub(crate) async fn resume(
    platform: &dyn PlatformContext,
    db: &DatabaseConnection,
    config: &AgentConfig,
    inputs: ServedInputs,
    base_dir: &Path,
    run: ServedRun<'_>,
    input: ResumeInput,
) -> StartedPipeline {
    let ctx = build_context(platform, config, inputs);
    let built = build_react_run(platform, config, ctx, base_dir).await;
    let prior = match run.thread_id {
        Some(tid) => prior_turns(db, tid, run.run_id).await,
        None => Vec::new(),
    };
    // An unreadable log reads as empty here: `resume_loop` reads it again
    // and reports the failure itself (one `error` row).
    let log = read_log(db, run.run_id).await.unwrap_or_default();
    let resumed = Resumed {
        run_id: run.run_id.to_string(),
        question: run.question,
        prior,
    };
    let handle = serve_resumed(built, db, resumed, &log, Some(input));
    StartedPipeline {
        run_id: run.run_id.to_string(),
        source_type: "analytics".to_string(),
        inner: ErasedHandle::React(handle),
        starting_driver: None,
    }
}

/// The run's rows as the runtime reads them, or why they could not be read.
pub(crate) type PriorLog = Result<Vec<(String, Value)>, String>;

/// Reads the run's log.
pub(crate) async fn read_log(db: &DatabaseConnection, run_id: &str) -> PriorLog {
    let rows = DbTranscriptStore::new(db.clone()).load(run_id).await;
    rows.map_err(|e| e.to_string())
}

/// What a start knows of an existing run's recorded `runtime`.
pub(crate) enum RecordedRuntime {
    /// The driver did not read the row: [`runtime_of`] looks it up.
    Unread,
    /// The driver read it (`None`: the row has no `runtime` key).
    Read(Option<String>),
}

/// The existing run's recorded `runtime`: what the driver read, else one
/// primary-key lookup. A lookup that fails reads as no record: the start
/// then decides as a first attempt does (`react_served::decide`), so an
/// unreadable-metadata run whose agent no longer opts in runs the FSM. A
/// read error never forces or stamps react.
pub(crate) async fn runtime_of(
    db: &DatabaseConnection,
    run_id: &str,
    recorded: &RecordedRuntime,
) -> Option<String> {
    if let RecordedRuntime::Read(runtime) = recorded {
        return runtime.clone();
    }
    match agentic_runtime::crud::get_run(db, run_id).await {
        Ok(run) => crate::queued_run::recorded_runtime(run?.metadata.as_ref()),
        Err(e) => {
            tracing::warn!(run_id, error = %e, "react: run metadata unreadable; deciding afresh");
            None
        }
    }
}

/// The log a start reads before it picks a runtime: only a run recorded
/// `runtime: react` can hold a react attempt (the stamp precedes its first
/// react row), so any other start reads nothing (`None`) and is decided as
/// before.
pub(crate) async fn prior_log<R, Fut>(recorded: Option<&str>, read: R) -> Option<PriorLog>
where
    R: FnOnce() -> Fut,
    Fut: std::future::Future<Output = PriorLog>,
{
    if recorded == Some(REACT) {
        Some(read().await)
    } else {
        None
    }
}

/// Whether a start continues a react attempt, whatever the flag or the
/// agent's key now say: its log holds `react_*` rows. Asked only of a run
/// recorded `react` ([`prior_log`]); there an unreadable log counts, so the
/// start fails closed on react (one `error` row) instead of running a fresh
/// FSM over rows it could not see. An FSM `done`/`error` alone does not.
pub(crate) fn holds_react(log: &PriorLog) -> bool {
    match log {
        Ok(rows) => has_react_rows(rows),
        Err(_) => true,
    }
}

/// The `runtime` stamp of a run that continues a react attempt: `react`,
/// whatever the agent's key or the flag now say.
pub(crate) fn react_stamp() -> Value {
    serde_json::json!({ RUNTIME_KEY: REACT })
}

fn has_react_rows(rows: &[(String, Value)]) -> bool {
    rows.iter().any(|(t, _)| t.starts_with("react_"))
}

/// Whether a start must continue the log rather than start over: it holds
/// an earlier attempt's react rows, or an end (`done`/`error`) already.
pub(crate) fn continues(log: &[(String, Value)]) -> bool {
    has_react_rows(log) || already_terminal(log).is_some()
}

/// The run a resume or a re-driven start continues.
pub(crate) struct Resumed {
    pub run_id: String,
    pub question: String,
    pub prior: Vec<PriorTurn>,
}

/// Serves a continuation of `run` from its log. `input` is the resume's
/// (`None`: a re-driven start, which continues with no answer). A failed
/// build on a log that already ended returns that end and writes nothing,
/// so a run never gets a second terminal row; otherwise it gets its one
/// `error` row.
pub(crate) fn serve_resumed(
    built: Result<ReactRun, String>,
    db: &DatabaseConnection,
    run: Resumed,
    log: &[(String, Value)],
    input: Option<ResumeInput>,
) -> RowHandle {
    let recorded = already_terminal(log);
    let store = DbTranscriptStore::new(db.clone());
    let input = input.unwrap_or_else(|| ResumeInput {
        run_id: run.run_id.clone(),
        answer: None,
        call_id: None,
    });
    let Resumed {
        run_id,
        question,
        prior,
    } = run;
    let asked = question.clone();
    serve_with(built, run_id, asked, move |built, ev, cancel| async move {
        let outcome = match built {
            Ok(run) => {
                let seed = Seed { prior, question };
                resume_react(run, &store, seed, input, ev, cancel).await
            }
            Err(reason) => recorded.unwrap_or(LoopOutcome::Refused { reason }),
        };
        without_markers(outcome)
    })
}

/// A `Done` with its citation markers stripped. A log that already ended
/// returns its recorded reply as written (the done hook, which strips, ran
/// on the first attempt and is not run again: it would emit `answer_result`
/// a second time). Stripping is idempotent, so a reply the hook already
/// stripped is unchanged.
fn without_markers(outcome: LoopOutcome) -> LoopOutcome {
    match outcome {
        LoopOutcome::Done { mut answer } => {
            answer.text = strip_citations(&answer.text);
            LoopOutcome::Done { answer }
        }
        other => other,
    }
}

/// What the transcript was seeded with at the start: the thread's prior
/// turns and the run's question (the system prompt is the rebuilt run's).
pub(crate) struct Seed {
    pub prior: Vec<PriorTurn>,
    pub question: String,
}

/// Resumes `run` from `store`. A resume refused with an answer (no call is
/// parked, or the answer names another call) is retried once with no answer,
/// so the run continues as a recovery would; refused again, it stays
/// `Refused` and the served handle writes its `error` row.
pub(crate) async fn resume_react(
    run: ReactRun,
    store: &dyn TranscriptStore,
    seed: Seed,
    input: ResumeInput,
    ev: EventSink,
    cancel: CancellationToken,
) -> LoopOutcome {
    let cfg = loop_config(&run, input.run_id.clone());
    let transcript = Transcript::seeded(run.system.clone(), &seed.prior, seed.question);
    let toolset = &run.analytics.toolset;
    let cx = || LoopCx {
        cfg: &cfg,
        ev: &ev,
        cancel: &cancel,
        nudge: None,
    };
    let had_answer = input.answer.is_some();
    let retry = ResumeInput {
        run_id: input.run_id.clone(),
        answer: None,
        call_id: input.call_id.clone(),
    };
    let first = resume_loop(
        &*run.engine,
        toolset,
        store,
        transcript.clone(),
        input,
        cx(),
    )
    .await;
    match first {
        LoopOutcome::Refused { reason } if had_answer => {
            tracing::warn!(
                run_id = %retry.run_id,
                %reason,
                "react resume refused its answer; continuing without it"
            );
            resume_loop(&*run.engine, toolset, store, transcript, retry, cx()).await
        }
        other => other,
    }
}

#[cfg(test)]
#[path = "react_resume_tests.rs"]
mod tests;
