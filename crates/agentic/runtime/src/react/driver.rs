//! The react loop driver: decide, dispatch, append, repeat.

use std::sync::{Arc, Mutex};

use agentic_core::events::CoreEvent;
use agentic_core::react::{
    Decision, DoneHook, Engine, Entry, EventSink, ResultHandle, Toolset, Transcript, Turn,
};
use agentic_core::tools::ToolDef;
use tokio_util::sync::CancellationToken;

use super::budget::BudgetMeter;
use super::dispatch::{DispatchCx, Dispatched, dispatch_turn};
use super::events::{emit_core, emit_turn};
use super::nudge::{Nudge, NudgeState};

/// Limits on one run, enforced by [`BudgetMeter`] between model turns.
#[derive(Clone, Debug)]
pub struct Budgets {
    pub max_turns: u32,
    pub max_tokens: u64,
    pub max_cost_usd: Option<f64>,
}

impl Default for Budgets {
    fn default() -> Self {
        Self {
            max_turns: 30,
            max_tokens: 2_000_000,
            max_cost_usd: None,
        }
    }
}

pub struct LoopConfig {
    pub budgets: Budgets,
    pub run_id: String,
    /// Handed to every tool through [`ToolCx`].
    pub results: Arc<dyn ResultHandle>,
    /// Called on every `Done`, before the `done` row. `None`: no hook.
    pub on_done: Option<Arc<dyn DoneHook>>,
    /// A one-time note added to a tool result when the run looks stuck.
    /// `None`: no note.
    pub nudge: Option<Nudge>,
}

#[derive(Debug)]
pub enum LoopOutcome {
    Done {
        answer: Turn,
    },
    Suspended {
        call_id: String,
        question: String,
        suggestions: Vec<String>,
    },
    Failed {
        message: String,
    },
    Cancelled,
    /// A resume that was not honoured and wrote no row (see `resume_loop`):
    /// the run's state is unchanged, so the caller reports `reason` to
    /// whoever delivered the resume and the run keeps its current status.
    Refused {
        reason: String,
    },
}

pub async fn run_loop(
    engine: &dyn Engine,
    toolset: &Toolset,
    transcript: Transcript,
    cfg: &LoopConfig,
    ev: &EventSink,
    cancel: &CancellationToken,
) -> LoopOutcome {
    let meter = BudgetMeter::new(cfg.budgets.clone());
    let start = LoopStart { transcript, meter };
    run_loop_from(engine, toolset, start, cfg, ev, cancel).await
}

/// Where a loop starts: a transcript and the budget already spent on it.
/// Resume pre-charges the turns recorded in the log (`LoopStart::resumed`).
pub(super) struct LoopStart {
    pub transcript: Transcript,
    pub meter: BudgetMeter,
}

pub(super) async fn run_loop_from(
    engine: &dyn Engine,
    toolset: &Toolset,
    start: LoopStart,
    cfg: &LoopConfig,
    ev: &EventSink,
    cancel: &CancellationToken,
) -> LoopOutcome {
    let LoopStart {
        mut transcript,
        mut meter,
    } = start;
    // Cancellation: this driver only returns `Cancelled` and writes no
    // terminal row. The worker/coordinator owns the `cancelled` outcome, as
    // for analytics (`PipelineOutcome::Cancelled`, no `CoreEvent::Error`).
    let defs: Vec<ToolDef> = toolset.tools.iter().map(|t| t.def()).collect();
    // Rebuilt from the transcript, so a resumed run neither forgets the
    // queries it made nor fires the note a second time.
    let nudge = (cfg.nudge.clone()).map(|n| Mutex::new(NudgeState::resumed(n, &transcript)));
    loop {
        if cancel.is_cancelled() {
            return LoopOutcome::Cancelled;
        }
        if let Some(message) = meter.exceeded() {
            return finish(ev, cfg, LoopOutcome::Failed { message }).await;
        }
        // Raced against cancel: a cancelled run must not wait out a stream
        // and its retries, nor write the turn it was deciding.
        let decide_start = std::time::Instant::now();
        let decided = tokio::select! {
            biased;
            _ = cancel.cancelled() => return LoopOutcome::Cancelled,
            r = engine.decide(&transcript, &defs, ev) => r,
        };
        let decide_ms = decide_start.elapsed().as_millis() as u64;
        let decision = match decided {
            Ok(d) => d,
            Err(e) => {
                let message = e.to_string();
                return finish(ev, cfg, LoopOutcome::Failed { message }).await;
            }
        };
        match decision {
            Decision::Reply(turn) => {
                emit_turn(ev, &turn, decide_ms).await;
                return finish(ev, cfg, LoopOutcome::Done { answer: turn }).await;
            }
            Decision::CallTools(turn) => {
                meter.charge(&turn.usage);
                emit_turn(ev, &turn, decide_ms).await;
                let calls = turn.tool_calls.clone();
                transcript.entries.push(Entry::Assistant(turn));
                let nudge = nudge.as_ref();
                let cx = DispatchCx {
                    cfg,
                    ev,
                    cancel,
                    nudge,
                };
                let dispatched = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return LoopOutcome::Cancelled,
                    d = dispatch_turn(toolset, &calls, cx) => d,
                };
                if let Some(s) = suspended(&mut transcript, dispatched) {
                    return s;
                }
            }
        }
    }
}

/// Appends the turn's results; `Some` when a suspend stopped it.
pub(super) fn suspended(transcript: &mut Transcript, d: Dispatched) -> Option<LoopOutcome> {
    transcript.entries.extend(d.entries);
    d.suspended.map(|s| LoopOutcome::Suspended {
        call_id: s.call_id,
        question: s.question,
        suggestions: s.suggestions,
    })
}

/// Emits the terminal row for `Done` / `Failed`; other outcomes pass through.
/// A `Done` runs the config's hook first, so its rows precede `done`.
pub(super) async fn finish(
    ev: &EventSink,
    cfg: &LoopConfig,
    mut outcome: LoopOutcome,
) -> LoopOutcome {
    let trace_id = cfg.run_id.clone();
    match &mut outcome {
        LoopOutcome::Done { answer } => {
            if let Some(hook) = &cfg.on_done {
                hook.on_done(answer, ev).await;
            }
            emit_core(ev, CoreEvent::Done { trace_id }).await
        }
        LoopOutcome::Failed { message } => {
            let message = message.clone();
            emit_core(ev, CoreEvent::Error { message, trace_id }).await
        }
        _ => {}
    }
    outcome
}
