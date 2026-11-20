//! [`LlmEngine`]: one model turn over any [`LlmProvider`](crate::LlmProvider),
//! behind the react loop's [`Engine`] contract.
//!
//! # Events
//!
//! Per [`Engine::decide`] the engine emits one `LlmStart`, then the thinking
//! and `LlmToken` events of every stream it opens. It emits no `LlmEnd`: the
//! runtime driver closes each returned turn with `LlmEnd` beside its
//! `react_turn` row, so emitting one here would double it. A retried attempt
//! re-streams from scratch, so its tokens may repeat on the event stream;
//! only one [`Turn`] is ever returned.
//!
//! # Continued turns
//!
//! A thinking-only turn with no text is continued: its blocks and a
//! "Continue." user message go back to the model for another stream. The
//! returned turn's `native` holds only that final stream, so the first
//! stream's thinking blocks and the "Continue." message are not persisted.
//! Both providers accept that history, so this is a narrow exception to
//! "thinking blocks come back verbatim".

use agentic_core::events::CoreEvent;
use agentic_core::react::{
    Decision, Engine, EngineError, EventSink, ToolCallReq, Transcript, Turn, TurnUsage,
};
use agentic_core::tools::ToolDef;
use async_trait::async_trait;
use serde_json::{Value, json};

use crate::constants::{DEFAULT_MAX_TOKENS, THINKING_MAX_TOKENS};
use crate::{ContentBlock, LlmClient, StopReason, ThinkingConfig, Usage, pricing, retry};

mod call;
mod fold;
mod ids;
#[cfg(test)]
mod mock;
mod render;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_ids;
mod trace;

use call::{Attempt, RetryBudget};
use fold::{Folded, STATE};

/// How an [`LlmEngine`] runs each turn.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub thinking: ThinkingConfig,
    /// Times a thinking-only turn with no text is continued ("Continue.").
    pub max_continuations: u32,
    pub max_rate_limit_retries: u32,
    pub max_transient_retries: u32,
    /// Dynamic system-prompt suffix kept out of the Anthropic cache prefix
    /// (`Today is YYYY-MM-DD.`); the same value `run_with_tools` takes as
    /// `ToolLoopConfig::system_date_hint`.
    pub system_date_hint: Option<String>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            thinking: ThinkingConfig::Disabled,
            max_continuations: 2,
            max_rate_limit_retries: retry::MAX_RATE_LIMIT_RETRIES,
            max_transient_retries: retry::MAX_TRANSIENT_RETRIES,
            system_date_hint: None,
        }
    }
}

/// The real [`Engine`]: renders the transcript, streams one model turn and
/// prices it.
pub struct LlmEngine {
    client: LlmClient,
    cfg: EngineConfig,
}

/// State of one `decide` across its streams.
struct TurnRun {
    messages: Vec<Value>,
    max_tokens: Option<u32>,
    usage: Usage,
    round: u32,
    continuations: u32,
    max_tokens_retried: bool,
}

enum Step {
    Done(Decision),
    Again,
}

impl LlmEngine {
    pub fn new(client: LlmClient, cfg: EngineConfig) -> Self {
        Self { client, cfg }
    }

    /// Decides what to do with a finished stream: return, or stream again.
    fn next_step(
        &self,
        run: &mut TurnRun,
        mut f: Folded,
        t: &Transcript,
    ) -> Result<Step, EngineError> {
        if f.tool_calls.is_empty() && f.text.trim().is_empty() && has_thinking(&f) {
            if run.continuations >= self.cfg.max_continuations {
                return Err(EngineError::Fatal(
                    "model produced reasoning but no text output; continuations exhausted".into(),
                ));
            }
            self.continue_thinking(run, f);
            return Ok(Step::Again);
        }
        if f.tool_calls.is_empty() && f.usage.stop_reason == StopReason::MaxTokens {
            if run.max_tokens_retried {
                return Err(EngineError::MaxTokens {
                    partial_text: f.text,
                });
            }
            run.max_tokens_retried = true;
            run.max_tokens = Some(self.cap(run.max_tokens).saturating_mul(2));
            return Ok(Step::Again);
        }
        let (taken, turn_index) = ids::taken_ids(t);
        ids::ensure_unique(&mut f, taken, turn_index);
        Ok(Step::Done(self.decision(f, &run.usage)))
    }

    /// Appends the thinking-only turn and a "Continue." nudge, as
    /// `tool_loop.rs` does. Anthropic rejects an assistant message whose last
    /// block is thinking, so an empty text block closes it.
    fn continue_thinking(&self, run: &mut TurnRun, f: Folded) {
        let mut blocks = f.ordered;
        blocks.push(ContentBlock::Text {
            text: String::new(),
        });
        let provider = self.client.provider();
        run.messages.push(provider.assistant_message(&blocks));
        run.messages
            .push(json!({"role": "user", "content": "Continue."}));
        if f.usage.stop_reason == StopReason::MaxTokens {
            run.max_tokens = Some(self.cap(run.max_tokens).saturating_mul(2));
        }
        run.continuations += 1;
    }

    /// The `max_tokens` in force: the override, else the provider default
    /// for this thinking config (as `AnthropicProvider` computes it).
    fn cap(&self, current: Option<u32>) -> u32 {
        current.unwrap_or(match &self.cfg.thinking {
            ThinkingConfig::Disabled => DEFAULT_MAX_TOKENS,
            ThinkingConfig::Adaptive | ThinkingConfig::Effort(_) => THINKING_MAX_TOKENS,
            ThinkingConfig::Manual { budget_tokens } => {
                THINKING_MAX_TOKENS.max(budget_tokens.saturating_add(DEFAULT_MAX_TOKENS))
            }
        })
    }

    /// The final turn, priced on the usage of every stream of this `decide`.
    fn decision(&self, f: Folded, usage: &Usage) -> Decision {
        let provider = self.client.provider();
        let native = provider.assistant_message(&f.blocks());
        let tool_calls: Vec<ToolCallReq> = f
            .tool_calls
            .into_iter()
            .map(|tc| ToolCallReq {
                id: tc.id,
                name: tc.name,
                input: tc.input,
            })
            .collect();
        let turn = Turn {
            text: f.text,
            tool_calls,
            native,
            usage: turn_usage(provider.model_name(), usage),
        };
        if turn.tool_calls.is_empty() {
            Decision::Reply(turn)
        } else {
            Decision::CallTools(turn)
        }
    }
}

/// Thinking happened: summary text, or a raw block. Anthropic yields a
/// Thinking block with an empty summary when the display is omitted, and a
/// RedactedThinking block with no summary chunk at all.
fn has_thinking(f: &Folded) -> bool {
    !f.thinking.is_empty()
        || f.ordered.iter().any(|b| {
            matches!(
                b,
                ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. }
            )
        })
}

fn turn_usage(model: &str, u: &Usage) -> TurnUsage {
    let (input, output) = (u.input_tokens as u64, u.output_tokens as u64);
    let (write, read) = (
        u.cache_creation_input_tokens as u64,
        u.cache_read_input_tokens as u64,
    );
    TurnUsage {
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: read,
        cache_write_tokens: write,
        cost_usd: pricing::cost_for_call(model, input, output, write, read),
        model: model.to_string(),
    }
}

fn add_usage(total: &mut Usage, u: &Usage) {
    total.input_tokens += u.input_tokens;
    total.output_tokens += u.output_tokens;
    total.cache_creation_input_tokens += u.cache_creation_input_tokens;
    total.cache_read_input_tokens += u.cache_read_input_tokens;
    total.stop_reason = u.stop_reason.clone();
}

#[async_trait]
impl Engine for LlmEngine {
    async fn decide(
        &self,
        transcript: &Transcript,
        tools: &[ToolDef],
        events: &EventSink,
    ) -> Result<Decision, EngineError> {
        let system = transcript.system.as_str();
        let messages = render::render(self.client.provider(), transcript);
        let start = CoreEvent::LlmStart {
            state: STATE.into(),
            prompt_tokens: fold::estimate_prompt_tokens(system, &messages),
            sub_spec_index: None,
        };
        fold::emit(events, start).await;
        let mut run = TurnRun {
            messages,
            max_tokens: None,
            usage: Usage::default(),
            round: 0,
            continuations: 0,
            max_tokens_retried: false,
        };
        let mut budget = RetryBudget::default();
        loop {
            let attempt = Attempt {
                system,
                messages: &run.messages,
                tools,
                max_tokens: run.max_tokens,
                round: run.round,
            };
            let f = self.call_with_retry(&attempt, &mut budget, events).await?;
            run.round += 1;
            add_usage(&mut run.usage, &f.usage);
            if let Step::Done(d) = self.next_step(&mut run, f, transcript)? {
                return Ok(d);
            }
        }
    }
}
