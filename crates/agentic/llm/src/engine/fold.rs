//! Folding one provider stream into the parts of a [`Turn`].
//!
//! This mirrors the per-round fold in `crate::client::tool_loop`
//! (`run_with_tools`); keep the two in step. It is a copy rather than a shared
//! helper because the tool loop's fold also emits its own `LlmEnd`, suppresses
//! tokens under a response schema and writes to a typed `EventStream`, none
//! of which the react engine wants.
//!
//! [`Turn`]: agentic_core::react::Turn

use agentic_core::events::{CoreEvent, Event};
use agentic_core::react::EventSink;
use serde_json::Value;

use crate::genai::ChunkStream;
use crate::{Chunk, ContentBlock, LlmError, ToolCallChunk, Usage};

/// The event-row `state` for every core event the engine emits.
pub(super) const STATE: &str = "react";

/// What one stream produced.
#[derive(Debug, Default)]
pub(super) struct Folded {
    pub text: String,
    pub thinking: String,
    pub tool_calls: Vec<ToolCallChunk>,
    /// Raw blocks and tool calls in stream order (no text).
    pub ordered: Vec<ContentBlock>,
    /// Every block in stream order, text included: the replay order when the
    /// stream carries advisor blocks.
    pub in_order: Vec<ContentBlock>,
    pub usage: Usage,
}

impl Folded {
    /// The assistant blocks, text first and then the stream-ordered blocks,
    /// under the same rule as `tool_loop.rs` (Anthropic wants text before
    /// tool_use; OpenAI's `assistant_message` reorders text itself).
    ///
    /// A stream with advisor blocks keeps its own order instead: a paused
    /// turn must come back unchanged, and text written after the advice
    /// cannot move ahead of the call. Empty text blocks are dropped, which
    /// the API rejects.
    pub fn blocks(&self) -> Vec<ContentBlock> {
        let advised = self
            .ordered
            .iter()
            .any(|b| matches!(b, ContentBlock::ServerTool { .. }));
        if advised {
            return self
                .in_order
                .iter()
                .filter(|b| !matches!(b, ContentBlock::Text { text } if text.is_empty()))
                .cloned()
                .collect();
        }
        let mut blocks = Vec::with_capacity(self.ordered.len() + 1);
        if !self.text.is_empty() {
            blocks.push(ContentBlock::Text {
                text: self.text.clone(),
            });
        }
        blocks.extend(self.ordered.iter().cloned());
        blocks
    }
}

/// Sends one core event as an `(event_type, payload)` row; a closed sink is
/// logged, never fatal (the runtime's `react/events.rs` does the same).
pub(super) async fn emit(ev: &EventSink, e: CoreEvent) {
    if ev.send(Event::<()>::Core(e).serialize()).await.is_err() {
        tracing::warn!("react engine: event sink closed; row dropped");
    }
}

/// Consumes `stream`, streaming thinking and text events to `ev`. A stream
/// error closes any open thinking block and is returned.
pub(super) async fn fold(stream: ChunkStream, ev: &EventSink) -> Result<Folded, LlmError> {
    use tokio_stream::StreamExt as _;
    let mut out = Folded::default();
    let mut in_thinking = false;
    let mut s = std::pin::pin!(stream);
    let mut result = Ok(());
    while let Some(chunk) = s.next().await {
        match chunk {
            Ok(c) => on_chunk(&mut out, &mut in_thinking, c, ev).await,
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }
    if in_thinking {
        emit(ev, thinking_end()).await;
    }
    result.map(|()| out)
}

async fn on_chunk(out: &mut Folded, in_thinking: &mut bool, chunk: Chunk, ev: &EventSink) {
    match chunk {
        Chunk::ThinkingSummary(t) => {
            if !*in_thinking {
                *in_thinking = true;
                let start = CoreEvent::ThinkingStart {
                    state: STATE.into(),
                    sub_spec_index: None,
                };
                emit(ev, start).await;
            }
            out.thinking.push_str(&t);
            if !t.is_empty() {
                let token = CoreEvent::ThinkingToken {
                    token: t,
                    sub_spec_index: None,
                };
                emit(ev, token).await;
            }
        }
        Chunk::Text(t) => {
            if *in_thinking {
                *in_thinking = false;
                emit(ev, thinking_end()).await;
            }
            out.text.push_str(&t);
            match out.in_order.last_mut() {
                Some(ContentBlock::Text { text }) => text.push_str(&t),
                _ => out.in_order.push(ContentBlock::Text { text: t.clone() }),
            }
            if !t.is_empty() {
                let token = CoreEvent::LlmToken {
                    token: t,
                    sub_spec_index: None,
                };
                emit(ev, token).await;
            }
        }
        Chunk::ToolCall(tc) => {
            let block = ContentBlock::ToolUse {
                id: tc.id.clone(),
                name: tc.name.clone(),
                input: tc.input.clone(),
                provider_data: tc.provider_data.clone(),
            };
            out.in_order.push(block.clone());
            out.ordered.push(block);
            out.tool_calls.push(tc);
        }
        Chunk::RawBlock(block) => {
            out.in_order.push(block.clone());
            out.ordered.push(block);
        }
        Chunk::Done(u) => out.usage = u,
    }
}

fn thinking_end() -> CoreEvent {
    CoreEvent::ThinkingEnd {
        state: STATE.into(),
        sub_spec_index: None,
    }
}

/// Rough prompt-token estimate for `LlmStart` (characters / 4), the same
/// heuristic `run_with_tools` uses.
pub(super) fn estimate_prompt_tokens(system: &str, messages: &[Value]) -> usize {
    let body: usize = messages.iter().map(|m| m.to_string().len()).sum();
    (system.len() + body) / 4
}
