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
//!
//! # Paused turns
//!
//! A stream that stops on `pause_turn` (an advisor call still pending) goes
//! back as the assistant message with no user message, and the model carries
//! on. Unlike a continued turn, every paused stream is part of the returned
//! turn: its blocks and text come first in `native` and `text`.

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
mod tests_advisor;
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
    /// The agent's `llm.max_tokens`: the output cap of every request, in place
    /// of the provider's default. `None` keeps the default. The one retry on a
    /// truncated turn doubles whatever is in force.
    pub max_tokens: Option<u32>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            thinking: ThinkingConfig::Disabled,
            max_continuations: 2,
            max_rate_limit_retries: retry::MAX_RATE_LIMIT_RETRIES,
            max_transient_retries: retry::MAX_TRANSIENT_RETRIES,
            system_date_hint: None,
            max_tokens: None,
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
    /// Blocks and text of the streams that stopped on `pause_turn`.
    paused: Vec<ContentBlock>,
    paused_text: String,
    pauses: usize,
}

/// Paused streams one turn may resume before it is abandoned.
const MAX_PAUSES: usize = 8;

enum Step {
    Done(Box<Decision>),
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
        // Before the thinking-only check: a paused stream often holds only
        // thinking and the advisor call, and must not get a "Continue.".
        if f.usage.stop_reason == StopReason::PauseTurn {
            self.resume_paused(run, f)?;
            return Ok(Step::Again);
        }
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
        Ok(Step::Done(Box::new(self.decision(f, run))))
    }

    /// Appends a paused stream as the assistant message, unchanged, and keeps
    /// it for the returned turn.
    fn resume_paused(&self, run: &mut TurnRun, f: Folded) -> Result<(), EngineError> {
        if run.pauses >= MAX_PAUSES {
            return Err(EngineError::Fatal(format!(
                "the model paused {MAX_PAUSES} times in one turn waiting on the advisor"
            )));
        }
        let blocks = f.blocks();
        let provider = self.client.provider();
        run.messages.push(provider.assistant_message(&blocks));
        run.paused.extend(blocks);
        run.paused_text.push_str(&f.text);
        run.pauses += 1;
        Ok(())
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
    /// Paused streams come first in its blocks and text.
    fn decision(&self, f: Folded, run: &TurnRun) -> Decision {
        let provider = self.client.provider();
        let mut blocks = run.paused.clone();
        blocks.extend(f.blocks());
        let native = provider.assistant_message(&blocks);
        let text = format!("{}{}", run.paused_text, f.text);
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
            text,
            tool_calls,
            native,
            usage: turn_usage(provider.model_name(), &run.usage),
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
    let executor = pricing::cost_for_call(model, input, output, write, read);
    let a = &u.advisor;
    let cost_usd = if a.calls == 0 {
        executor
    } else {
        executor.map(|e| e + advisor_cost(a))
    };
    TurnUsage {
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: read,
        cache_write_tokens: write,
        cost_usd,
        model: model.to_string(),
        advisor_calls: a.calls,
        advisor_input_tokens: (a.input_tokens
            + a.cache_creation_input_tokens
            + a.cache_read_input_tokens) as u64,
        advisor_output_tokens: a.output_tokens as u64,
    }
}

/// Priced at the dearest Claude rate ($15/$75) when the advisor model is not in the
/// table, so the react cost cap over-counts an unknown advisor rather than
/// letting its spend through as zero.
const UNPRICED_ADVISOR_RATES_OF: &str = "claude-opus-4-1";

/// The advisor calls' cost at the advisor model's rates.
fn advisor_cost(a: &crate::AdvisorUsage) -> f64 {
    let tokens = |model: &str| {
        pricing::cost_for_call(
            model,
            a.input_tokens as u64,
            a.output_tokens as u64,
            a.cache_creation_input_tokens as u64,
            a.cache_read_input_tokens as u64,
        )
    };
    tokens(&a.model).unwrap_or_else(|| {
        tracing::warn!(
            model = %a.model,
            "advisor model has no price; costing it at {UNPRICED_ADVISOR_RATES_OF} rates"
        );
        tokens(UNPRICED_ADVISOR_RATES_OF).unwrap_or(0.0)
    })
}

fn add_usage(total: &mut Usage, u: &Usage) {
    total.input_tokens += u.input_tokens;
    total.output_tokens += u.output_tokens;
    total.cache_creation_input_tokens += u.cache_creation_input_tokens;
    total.cache_read_input_tokens += u.cache_read_input_tokens;
    total.stop_reason = u.stop_reason.clone();
    total.advisor.add(&u.advisor);
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
            paused: Vec::new(),
            paused_text: String::new(),
            pauses: 0,
            messages,
            max_tokens: self.cfg.max_tokens,
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
                return Ok(*d);
            }
        }
    }
}
