//! The reflection turn: after an answer that took work, one more model turn
//! that may save notes. The runtime triggers it, because a prompt rule alone
//! may never fire (the advisor rule on BIRD: 66 runs made 3 or more queries,
//! 9 called the advisor).

use std::time::Instant;

use agentic_core::events::{CoreEvent, Outcome};
use agentic_core::react::{Decision, Engine, Entry, Toolset, Transcript, Turn};
use agentic_core::tools::ToolDef;

use crate::react::dispatch::{DispatchCx, dispatch_turn};
use crate::react::events::{emit_core, emit_turn_as};
use crate::react::nudge::results_of;

use super::MemoryGate;

/// The user turn that asks for notes.
pub const REFLECT_PROMPT: &str = "Before this run ends: is anything here worth remembering \
    for the next question in this workspace? That is, which view, measure or filter answers \
    this kind of question, a query shape that failed and the one that worked, or what the \
    user corrected. Never a number from a result. Call `remember` once per fact, each note \
    at most 400 characters (or `forget` for a note that proved wrong), or reply \"no\".";

/// The `LlmEnd.state` of a reflection turn, so readers can count it apart
/// from the answer's turns.
pub const REFLECT_STATE: &str = "react_reflect";

/// When and how a run reflects. Triggered by `after_calls` recorded results of
/// `query_tool`, a failed one, or an answered `ask_user`.
///
/// The turn is offered the run's whole toolset, so the request keeps the
/// run's prompt cache (tools are the first part of the cached prefix), and
/// only the calls named in `only` run.
#[derive(Clone, Debug)]
pub struct Reflect {
    /// The tools whose calls the reflection turn runs (`remember`, `forget`).
    pub only: Vec<String>,
    pub query_tool: String,
    pub after_calls: usize,
    pub prompt: String,
    /// Opened while the turn's memory calls run; shut otherwise.
    pub gate: MemoryGate,
}

impl Reflect {
    fn due(&self, transcript: &Transcript) -> bool {
        let queries = results_of(transcript, &self.query_tool);
        queries.len() >= self.after_calls
            || queries.iter().any(|(_, is_error)| *is_error)
            || !results_of(transcript, "ask_user").is_empty()
    }
}

/// Runs the config's reflection turn after `answer`, when one is due: one
/// model turn over the run's toolset, whose calls to the [`Reflect::only`]
/// tools run once. It never changes the answer or the outcome: an engine
/// error is logged and the run still ends `Done`. The turn sits between a
/// `StateEnter` and a `StateExit` named [`REFLECT_STATE`], so a reader (the
/// eval's metrics) can keep it apart from the answer.
pub(crate) async fn reflect(
    run: (&dyn Engine, &Toolset),
    cx: DispatchCx<'_>,
    transcript: &Transcript,
    answer: &Turn,
) {
    let Some(r) = cx.cfg.reflect.as_ref().filter(|r| r.due(transcript)) else {
        return;
    };
    let (ev, trace_id) = (cx.ev, cx.cfg.run_id.clone());
    let state = REFLECT_STATE.to_string();
    let (revision, sub_spec_index) = (0, None);
    let enter = CoreEvent::StateEnter {
        state: state.clone(),
        revision,
        trace_id: trace_id.clone(),
        sub_spec_index,
    };
    emit_core(ev, enter).await;
    reflect_turn(run, cx, r, transcript, answer).await;
    let outcome = Outcome::Advanced;
    let exit = CoreEvent::StateExit {
        state,
        outcome,
        trace_id,
        sub_spec_index,
    };
    emit_core(ev, exit).await;
}

async fn reflect_turn(
    (engine, toolset): (&dyn Engine, &Toolset),
    cx: DispatchCx<'_>,
    r: &Reflect,
    transcript: &Transcript,
    answer: &Turn,
) {
    let mut t = transcript.clone();
    t.entries.push(Entry::Assistant(answer.clone()));
    t.entries.push(Entry::User(r.prompt.clone()));
    let defs: Vec<ToolDef> = toolset.tools.iter().map(|t| t.def()).collect();
    let start = Instant::now();
    let decided = tokio::select! {
        biased;
        _ = cx.cancel.cancelled() => return,
        d = engine.decide(&t, &defs, cx.ev) => d,
    };
    let ms = start.elapsed().as_millis() as u64;
    let turn = match decided {
        Ok(Decision::Reply(turn) | Decision::CallTools(turn)) => turn,
        Err(e) => return tracing::warn!(error = %e, "react memory reflection failed"),
    };
    emit_turn_as(cx.ev, &turn, ms, REFLECT_STATE).await;
    let calls: Vec<_> = (turn.tool_calls.into_iter())
        .filter(|c| r.only.contains(&c.name))
        .collect();
    if calls.is_empty() {
        return;
    }
    let cancel = cx.cancel;
    r.gate.set_open(true);
    tokio::select! {
        biased;
        _ = cancel.cancelled() => {}
        _ = dispatch_turn(toolset, &calls, cx) => {}
    }
    r.gate.set_open(false);
}
