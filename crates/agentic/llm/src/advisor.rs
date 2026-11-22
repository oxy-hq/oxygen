//! Anthropic's advisor tool (beta `advisor-tool-2026-03-01`): a stronger
//! model the executor consults mid-turn for a plan or a check.
//!
//! What the API does, and what the client owes it:
//!
//! - The tool is declared in `tools` like any other, but Anthropic runs it. A
//!   call shows up as a `server_tool_use` block (input always `{}`) and an
//!   `advisor_tool_result` block that arrives whole in `content_block_start`.
//!   Both are passed back verbatim, in stream order, on every later request.
//! - The advice of the current advisor models is encrypted
//!   (`advisor_redacted_result`): it can be replayed, never read or logged.
//! - `stop_reason: "pause_turn"` means the call is still pending: resend with
//!   the response appended as the assistant message and no user message.
//! - Advisor tokens are reported only in `usage.iterations` entries of type
//!   `advisor_message`; the top-level usage counts the executor alone.

use serde_json::{Value, json};

use crate::{AdvisorUsage, ContentBlock};

/// The beta flag the advisor tool needs on every request that declares it
/// or replays its blocks.
pub(crate) const ADVISOR_BETA: &str = "advisor-tool-2026-03-01";

/// An advisor for an Anthropic executor.
#[derive(Debug, Clone, PartialEq)]
pub struct AdvisorConfig {
    /// The advisor model, at least as capable as the executor (the API
    /// returns 400 for a pair it does not accept).
    pub model: String,
    /// Advisor calls allowed per turn; past it the call returns
    /// `max_uses_exceeded` and the executor continues without advice.
    pub max_uses: Option<u32>,
    /// Cap on each call's output (thinking + text); 1024 at least.
    pub max_tokens: Option<u32>,
}

impl AdvisorConfig {
    /// The `tools` entry. The advisor's own prompt cache is always on, and
    /// never toggled: changing the tool mid-conversation edits `tools`,
    /// which invalidates replayed thinking blocks.
    pub(crate) fn tool_json(&self) -> Value {
        let mut tool = json!({
            "type": "advisor_20260301",
            "name": "advisor",
            "model": self.model,
            "caching": {"type": "ephemeral", "ttl": "5m"},
        });
        if let Some(n) = self.max_uses {
            tool["max_uses"] = json!(n);
        }
        if let Some(n) = self.max_tokens {
            tool["max_tokens"] = json!(n);
        }
        tool
    }
}

/// Whether a streamed content block is one the provider runs (and the
/// client must replay verbatim): `server_tool_use` and any `*_tool_result`.
pub(crate) fn is_server_tool_block(block_type: &str) -> bool {
    block_type == "server_tool_use" || block_type.ends_with("_tool_result")
}

/// The advisor calls in a `usage` object's `iterations`. The list is
/// cumulative for the response, so the last one seen replaces earlier ones.
pub(crate) fn advisor_usage(usage: &Value) -> Option<AdvisorUsage> {
    let iterations = usage.get("iterations")?.as_array()?;
    let tokens = |it: &Value, key: &str| it[key].as_u64().unwrap_or(0) as usize;
    let mut out = AdvisorUsage::default();
    for it in iterations
        .iter()
        .filter(|it| it["type"] == "advisor_message")
    {
        out.calls += 1;
        if out.model.is_empty() {
            out.model = it["model"].as_str().unwrap_or("").to_string();
        }
        out.input_tokens += tokens(it, "input_tokens");
        out.output_tokens += tokens(it, "output_tokens");
        out.cache_creation_input_tokens += tokens(it, "cache_creation_input_tokens");
        out.cache_read_input_tokens += tokens(it, "cache_read_input_tokens");
    }
    Some(out)
}

/// A server-tool (advisor) block being streamed: the block as it opened, and
/// any streamed input. Kept out of `AnthropicProvider::stream` so that
/// function does not grow a branch per server-tool event.
#[derive(Default)]
pub(crate) struct ServerBlock {
    block: Option<Value>,
    input: String,
}

impl ServerBlock {
    /// Opens a block when `cb` is a server-tool block; ignores anything else.
    pub(crate) fn open(&mut self, cb: &Value) {
        if is_server_tool_block(cb["type"].as_str().unwrap_or("")) {
            self.block = Some(cb.clone());
            self.input.clear();
        }
    }

    /// Takes a streamed input part while a server-tool block is open; any
    /// other part belongs to the client tool call in `tool_args`.
    pub(crate) fn push_input(&mut self, part: &str, tool_args: &mut String) {
        match self.block {
            Some(_) => self.input.push_str(part),
            None => tool_args.push_str(part),
        }
    }

    /// The finished block, with its streamed input, when one was open.
    pub(crate) fn close(&mut self) -> Option<ContentBlock> {
        let mut block = self.block.take()?;
        if let Ok(input) = serde_json::from_str::<Value>(&self.input) {
            block["input"] = input;
        }
        Some(ContentBlock::ServerTool {
            provider_data: block,
        })
    }
}

/// Folds a `message_delta` usage into the totals. With an advisor call the
/// executor's input grows after `message_start`, so the final count is here.
pub(crate) fn merge_delta_usage(
    usage: &Value,
    input_tokens: &mut usize,
    advisor: &mut AdvisorUsage,
) {
    if let Some(v) = usage["input_tokens"].as_u64() {
        *input_tokens = (*input_tokens).max(v as usize);
    }
    if let Some(a) = advisor_usage(usage) {
        *advisor = a;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_json_carries_the_optional_caps_only_when_set() {
        let mut cfg = AdvisorConfig {
            model: "claude-opus-5-5".into(),
            max_uses: None,
            max_tokens: None,
        };
        let t = cfg.tool_json();
        assert_eq!(t["type"], "advisor_20260301");
        assert_eq!(t["name"], "advisor");
        assert_eq!(t["model"], "claude-opus-5-5");
        assert_eq!(t["caching"]["type"], "ephemeral");
        assert!(t.get("max_uses").is_none() && t.get("max_tokens").is_none());
        cfg.max_uses = Some(3);
        cfg.max_tokens = Some(2048);
        let t = cfg.tool_json();
        assert_eq!(
            (t["max_uses"].as_u64(), t["max_tokens"].as_u64()),
            (Some(3), Some(2048))
        );
    }

    /// The docs' sample: executor iterations are not advisor usage.
    #[test]
    fn advisor_usage_sums_only_advisor_iterations() {
        let usage = json!({
            "input_tokens": 1760, "output_tokens": 531,
            "iterations": [
                {"type": "message", "input_tokens": 412, "output_tokens": 89},
                {"type": "advisor_message", "model": "claude-opus-5-5",
                 "input_tokens": 823, "cache_read_input_tokens": 0,
                 "cache_creation_input_tokens": 0, "output_tokens": 1612},
                {"type": "message", "input_tokens": 1348, "output_tokens": 442}
            ]
        });
        let a = advisor_usage(&usage).unwrap();
        assert_eq!(a.calls, 1);
        assert_eq!(a.model, "claude-opus-5-5");
        assert_eq!((a.input_tokens, a.output_tokens), (823, 1612));
        assert!(advisor_usage(&json!({"output_tokens": 5})).is_none());
    }

    #[test]
    fn server_tool_blocks_are_recognised_by_type() {
        assert!(is_server_tool_block("server_tool_use"));
        assert!(is_server_tool_block("advisor_tool_result"));
        assert!(!is_server_tool_block("tool_use"));
        assert!(!is_server_tool_block("text"));
    }
}
