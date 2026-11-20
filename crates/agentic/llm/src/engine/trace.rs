//! The `llm.usage` / `llm.output` events `run_with_tools` emits per round
//! (`client/tool_loop.rs`), with the same names and fields, so a react run
//! reads like an FSM run in the trace detail view. Emitted inside the
//! inference span, one pair per provider stream.

use super::fold::Folded;
use crate::client::LLM_OUTPUT_PREVIEW_MAX_CHARS;

pub(super) fn record_round(f: &Folded, model: &str, duration_ms: u64) {
    let u = &f.usage;
    tracing::info!(
        name: "llm.usage",
        is_visible = true,
        prompt_tokens = u.input_tokens as i64,
        completion_tokens = u.output_tokens as i64,
        total_tokens = (u.input_tokens + u.output_tokens) as i64,
        cache_creation_tokens = u.cache_creation_input_tokens as i64,
        cache_read_tokens = u.cache_read_input_tokens as i64,
        model = %model,
        duration_ms = duration_ms,
        stop_reason = %format!("{:?}", u.stop_reason),
    );
    let names: Vec<&str> = f.tool_calls.iter().map(|tc| tc.name.as_str()).collect();
    tracing::info!(
        name: "llm.output",
        is_visible = true,
        text = %preview(&f.text),
        tool_calls = %serde_json::to_string(&names).unwrap_or_default(),
    );
}

/// `text` cut at a char boundary, with its full length noted when cut.
fn preview(text: &str) -> String {
    if text.len() <= LLM_OUTPUT_PREVIEW_MAX_CHARS {
        return text.to_string();
    }
    let cut = text
        .char_indices()
        .take_while(|(i, _)| *i < LLM_OUTPUT_PREVIEW_MAX_CHARS)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    format!("{}… ({} chars)", &text[..cut], text.len())
}
