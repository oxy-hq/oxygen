//! The checks `resume_loop` makes before it runs anything: is the run already
//! finished, did it crash after its reply, and does the resume name the call
//! the run is parked on.

use agentic_core::react::{Effect, Entry, ToolCallReq, Toolset, Transcript, Turn, TurnUsage};
use serde_json::Value;

use super::dispatch::find;
use super::driver::LoopOutcome;
use super::events::ReactEvent;
use super::resume::ResumeInput;

/// The outcome a run already ended with, when its rows hold a `done` or
/// `error` row. The LAST such row wins, as in
/// `update_run_terminal_from_events`. `Done` carries the last recorded turn
/// (an empty turn if none parses); `Failed` carries the row's message.
pub fn already_terminal(rows: &[(String, Value)]) -> Option<LoopOutcome> {
    let (t, payload) = rows
        .iter()
        .rev()
        .find(|(t, _)| t == "done" || t == "error")?;
    if t == "error" {
        let message = payload
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        return Some(LoopOutcome::Failed { message });
    }
    let answer = rows
        .iter()
        .rev()
        .find_map(|(t, p)| match ReactEvent::from_row(t, p)? {
            ReactEvent::ReactTurn { turn } => Some(turn),
            _ => None,
        })
        .unwrap_or_else(empty_turn);
    Some(LoopOutcome::Done { answer })
}

fn empty_turn() -> Turn {
    Turn {
        text: String::new(),
        tool_calls: vec![],
        native: Value::Null,
        usage: TurnUsage::default(),
    }
}

/// The reply a run recorded but never finished (a crash between its
/// `react_turn` and `done`): the last entry after the seed is an assistant
/// turn with no calls.
pub(super) fn unfinished_reply(t: &Transcript, seed_len: usize) -> Option<Turn> {
    match t.entries.get(seed_len..)?.last()? {
        Entry::Assistant(turn) if turn.tool_calls.is_empty() => Some(turn.clone()),
        _ => None,
    }
}

/// The call an answer lands on: the log's last `react_suspended` call, still
/// pending. `Ok(None)` when no answer is delivered. Refused (no row written)
/// when an answer finds no parked call, or names a different call.
pub(super) fn parked_call(
    rows: &[(String, Value)],
    resume: &ResumeInput,
    pending: &[ToolCallReq],
) -> Result<Option<String>, LoopOutcome> {
    if resume.answer.is_none() {
        return Ok(None);
    }
    let last = rows
        .iter()
        .rev()
        .find(|(t, _)| t == "react_suspended")
        .and_then(|(_, p)| p.get("call_id")?.as_str().map(String::from));
    if let (Some(want), Some(parked)) = (&resume.call_id, &last)
        && want != parked
    {
        return Err(refused(format!(
            "answer is for call `{want}` but the run is parked on `{parked}`"
        )));
    }
    match last {
        Some(id) if pending.iter().any(|c| c.id == id) => Ok(Some(id)),
        _ => Err(refused("no suspended call is waiting for an answer".into())),
    }
}

fn refused(reason: String) -> LoopOutcome {
    LoopOutcome::Refused { reason }
}

/// A crash left a `Write` call with no recorded result: it is not re-run.
///
/// Only calls after the parked (suspend) call are known not to have
/// started: dispatch returns at the suspend before any later call runs.
/// Anything before it may have run even with no result in the log, because
/// result rows are queued to the event sink and reach Postgres on a later
/// flush. So a pending write before the parked call (or anywhere, when
/// nothing is parked) is refused rather than risk running it twice.
pub(super) fn pending_write(
    toolset: &Toolset,
    pending: &[ToolCallReq],
    parked: &Option<String>,
) -> Option<String> {
    pending
        .iter()
        .take_while(|c| Some(&c.id) != parked.as_ref())
        .any(|c| find(toolset, &c.name).is_some_and(|t| t.effect() == Effect::Write))
        .then(|| "cannot re-run a write call after a crash".to_string())
}
