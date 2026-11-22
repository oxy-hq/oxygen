//! Suspend and resume. A suspended run stores only a pointer; the event log
//! is the source of truth, so resume rebuilds the transcript from it and
//! never re-runs a call that has a recorded result.

use std::sync::Mutex;

use agentic_core::human_input::SuspendedRunData;
use agentic_core::react::{Engine, EventSink, ToolCallReq, Toolset, Transcript};
use serde_json::json;

use super::budget::BudgetMeter;
use super::dispatch::{DispatchCx, Dispatched, dispatch_turn, record_text};
use super::driver::{LoopConfig, LoopOutcome, LoopStart, finish, run_loop_from, suspended};
use super::nudge::NudgeState;
use super::resume_guard::{already_terminal, parked_call, pending_write, unfinished_reply};
use super::store::{TranscriptStore, rebuild};

/// What [`suspension_data`] records about a parked run.
pub struct SuspensionInput<'a> {
    pub run_id: &'a str,
    pub call_id: &'a str,
    pub question: &'a str,
    pub suggestions: &'a [String],
    pub original_input: &'a str,
    pub trace_id: &'a str,
}

/// The persisted pointer for a suspended react run: `stage_data` holds the
/// run id and the suspended call's id, nothing else (the log has the rest).
pub fn suspension_data(i: SuspensionInput<'_>) -> SuspendedRunData {
    SuspendedRunData {
        from_state: "react".into(),
        original_input: i.original_input.into(),
        trace_id: i.trace_id.into(),
        stage_data: json!({ "run_id": i.run_id, "call_id": i.call_id }),
        question: i.question.into(),
        suggestions: i.suggestions.to_vec(),
    }
}

/// `answer: None` is crash recovery: nothing is suspended, the calls with no
/// recorded result simply run. With `Some`, the answer lands on the call the
/// log's last `react_suspended` row names. `call_id` is the id the caller
/// read from `SuspendedRunData.stage_data.call_id`; when given, it must name
/// that parked call, so a duplicate delivery of an earlier resume cannot land
/// on a later question. `None` skips that check.
pub struct ResumeInput {
    pub run_id: String,
    pub answer: Option<String>,
    pub call_id: Option<String>,
}

/// Resumes a run from its log.
///
/// - Already finished (a `done`/`error` row): returns that outcome
///   (`Done` with the last recorded turn, or `Failed` with its message) and
///   writes nothing, so a duplicate or stale resume cannot flip the run.
/// - Crashed after its reply: emits only `done`; the model is not called.
/// - An answer with no parked call, or for a call it is not parked on:
///   `Refused`, no row written.
/// - A `Write` call left unrecorded by a crash: `Failed` (an `error` row).
pub async fn resume_loop(
    engine: &dyn Engine,
    toolset: &Toolset,
    store: &dyn TranscriptStore,
    seed: Transcript,
    resume: ResumeInput,
    cx: DispatchCx<'_>,
) -> LoopOutcome {
    let (cfg, ev) = (cx.cfg, cx.ev);
    let rows = match store.load(&resume.run_id).await {
        Ok(r) => r,
        Err(e) => return fail(ev, cfg, e.to_string()).await,
    };
    if let Some(ended) = already_terminal(&rows) {
        return ended;
    }
    let seed_len = seed.entries.len();
    let rebuilt = match rebuild(seed, &rows, Some(cfg.results.as_ref())) {
        Ok(r) => r,
        Err(e) => return fail(ev, cfg, e.to_string()).await,
    };
    if let Some(answer) = unfinished_reply(&rebuilt.transcript, seed_len) {
        return finish(ev, cfg, LoopOutcome::Done { answer }).await;
    }
    let parked = match parked_call(&rows, &resume, &rebuilt.pending_calls) {
        Ok(p) => p,
        Err(refused) => return refused,
    };
    if let Some(message) = pending_write(toolset, &rebuilt.pending_calls, &parked) {
        return fail(ev, cfg, message).await;
    }
    let pending = Pending {
        calls: rebuilt.pending_calls,
        parked,
        answer: resume.answer,
    };
    let meter =
        BudgetMeter::preloaded(cfg.budgets.clone(), &rebuilt.transcript.entries[seed_len..]);
    let start = LoopStart {
        transcript: rebuilt.transcript,
        meter,
    };
    continue_run(engine, toolset, start, pending, cx).await
}

/// The rebuilt run's unanswered calls and the answer for the parked one.
struct Pending {
    calls: Vec<ToolCallReq>,
    parked: Option<String>,
    answer: Option<String>,
}

/// Runs the pending calls (raced against cancel), then the loop.
async fn continue_run(
    engine: &dyn Engine,
    toolset: &Toolset,
    mut start: LoopStart,
    p: Pending,
    cx: DispatchCx<'_>,
) -> LoopOutcome {
    let (cfg, ev, cancel) = (cx.cfg, cx.ev, cx.cancel);
    // The pending calls count towards the nudge like any others: built from
    // the rebuilt transcript, as `run_loop_from` builds its own afterwards.
    let nudge = (cfg.nudge.clone()).map(|n| Mutex::new(NudgeState::resumed(n, &start.transcript)));
    let cx = DispatchCx {
        nudge: nudge.as_ref(),
        ..cx
    };
    let d = tokio::select! {
        biased;
        _ = cancel.cancelled() => return LoopOutcome::Cancelled,
        d = run_pending(toolset, &p.calls, &p.parked, &p.answer, &cx) => d,
    };
    if let Some(stopped) = suspended(&mut start.transcript, d) {
        return stopped;
    }
    run_loop_from(engine, toolset, start, cfg, ev, cancel).await
}

/// Runs the pending calls in call order. The answer takes the parked call's
/// place in that order; calls on either side go through `dispatch_turn`, so
/// reads still batch and a second suspend still stops the turn.
async fn run_pending(
    toolset: &Toolset,
    pending: &[ToolCallReq],
    parked: &Option<String>,
    answer: &Option<String>,
    cx: &DispatchCx<'_>,
) -> Dispatched {
    let again = || DispatchCx {
        cfg: cx.cfg,
        ev: cx.ev,
        cancel: cx.cancel,
        nudge: cx.nudge,
    };
    let Some(pos) = parked
        .as_ref()
        .and_then(|id| pending.iter().position(|c| &c.id == id))
    else {
        return dispatch_turn(toolset, pending, again()).await;
    };
    let mut out = dispatch_turn(toolset, &pending[..pos], again()).await;
    if out.suspended.is_some() {
        return out;
    }
    let text = answer.clone().unwrap_or_default();
    out.entries
        .push(record_text(cx.ev, &pending[pos], text, false, 0).await);
    let rest = dispatch_turn(toolset, &pending[pos + 1..], again()).await;
    out.entries.extend(rest.entries);
    out.suspended = rest.suspended;
    out
}

async fn fail(ev: &EventSink, cfg: &LoopConfig, message: String) -> LoopOutcome {
    finish(ev, cfg, LoopOutcome::Failed { message }).await
}
