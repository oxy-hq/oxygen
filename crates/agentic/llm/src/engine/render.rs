//! Rendering a [`Transcript`] into provider-native messages.

use agentic_core::react::{Entry, Transcript, Turn};
use serde_json::{Value, json};

use crate::{ContentBlock, LlmProvider};

/// The provider messages for `t`, in order:
/// - `User` is a plain user message (the shape `run_with_tools` sends);
/// - `Assistant` is its `native` value, verbatim; a `Null` native (a turn
///   seeded from thread history) is rendered from its text instead;
/// - a run of consecutive `ToolResult`s is one `tool_result_messages` call,
///   so Anthropic gets every result of a batch in one user message.
pub(super) fn render(p: &dyn LlmProvider, t: &Transcript) -> Vec<Value> {
    let mut out = Vec::with_capacity(t.entries.len());
    let mut results: Vec<(String, String, bool)> = Vec::new();
    for entry in &t.entries {
        if let Entry::ToolResult {
            call_id,
            text,
            is_error,
        } = entry
        {
            results.push((call_id.clone(), text.clone(), *is_error));
            continue;
        }
        flush(p, &mut results, &mut out);
        match entry {
            Entry::User(text) => out.push(json!({"role": "user", "content": text})),
            Entry::Assistant(turn) => out.push(assistant(p, turn)),
            Entry::ToolResult { .. } => unreachable!("handled above"),
        }
    }
    flush(p, &mut results, &mut out);
    out
}

fn assistant(p: &dyn LlmProvider, turn: &Turn) -> Value {
    if turn.native.is_null() {
        p.assistant_message(&[ContentBlock::Text {
            text: turn.text.clone(),
        }])
    } else {
        turn.native.clone()
    }
}

fn flush(p: &dyn LlmProvider, results: &mut Vec<(String, String, bool)>, out: &mut Vec<Value>) {
    if !results.is_empty() {
        out.extend(p.tool_result_messages(results));
        results.clear();
    }
}
