//! The scripted provider the engine tests run against: a copy of
//! `crate::tests::MockProvider`, extended to fail at open and to record what
//! it was sent. Test-only, not exported.

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use agentic_core::tools::ToolDef;
use futures_core::Stream;
use serde_json::{Value, json};

use crate::{Chunk, ContentBlock, LlmError, LlmProvider, ResponseSchema, ThinkingConfig};

// ── MockProvider (copied from `crate::tests`, extended; not exported) ──────

/// One scripted `stream` call: fail at open, or yield these chunks.
pub(super) enum Round {
    Open(LlmError),
    Chunks(Vec<Result<Chunk, LlmError>>),
}

#[derive(Default)]
pub(super) struct Seen {
    pub messages: Vec<Vec<Value>>,
    pub max_tokens: Vec<Option<u32>>,
}

/// Pops one [`Round`] per `stream` call and records what it was sent.
pub(super) struct MockProvider {
    pub rounds: Mutex<VecDeque<Round>>,
    pub seen: Arc<Mutex<Seen>>,
}

#[async_trait::async_trait]
impl LlmProvider for MockProvider {
    async fn stream(
        &self,
        _system: &str,
        _system_date_suffix: &str,
        messages: &[Value],
        _tools: &[ToolDef],
        _thinking: &ThinkingConfig,
        _response_schema: Option<&ResponseSchema>,
        max_tokens_override: Option<u32>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Chunk, LlmError>> + Send>>, LlmError> {
        {
            let mut seen = self.seen.lock().unwrap();
            seen.messages.push(messages.to_vec());
            seen.max_tokens.push(max_tokens_override);
        }
        let round = self.rounds.lock().unwrap().pop_front();
        match round {
            Some(Round::Open(e)) => Err(e),
            Some(Round::Chunks(c)) => Ok(Box::pin(tokio_stream::iter(c))),
            None => Ok(Box::pin(tokio_stream::iter(Vec::new()))),
        }
    }

    fn assistant_message(&self, blocks: &[ContentBlock]) -> Value {
        assistant(blocks)
    }

    fn tool_result_messages(&self, results: &[(String, String, bool)]) -> Vec<Value> {
        tool_results(results)
    }

    fn model_name(&self) -> &str {
        "claude-sonnet-4-6"
    }
}

/// Encodes every block, in order, so ordering assertions are not vacuous.
pub(super) fn assistant(blocks: &[ContentBlock]) -> Value {
    let content: Vec<Value> = blocks
        .iter()
        .map(|b| match b {
            ContentBlock::Text { text } => json!({"type": "text", "text": text}),
            ContentBlock::Thinking { provider_data } => {
                json!({"type": "thinking", "data": provider_data})
            }
            ContentBlock::RedactedThinking { provider_data } => {
                json!({"type": "redacted", "data": provider_data})
            }
            ContentBlock::ToolUse { id, name, .. } => {
                json!({"type": "tool_use", "id": id, "name": name})
            }
        })
        .collect();
    json!({"role": "assistant", "content": content})
}

/// Anthropic-style: one user message carrying the whole batch.
pub(super) fn tool_results(results: &[(String, String, bool)]) -> Vec<Value> {
    let blocks: Vec<Value> = results
        .iter()
        .map(|(id, text, err)| {
            json!({"type": "tool_result", "tool_use_id": id, "content": text, "is_error": err})
        })
        .collect();
    vec![json!({"role": "user", "content": blocks})]
}
