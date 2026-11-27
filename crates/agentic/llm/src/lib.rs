//! LLM provider abstraction: Anthropic and OpenAI with token-level streaming.
//!
//! # Streaming
//!
//! [`LlmProvider::stream`] returns a [`Stream`] of [`Chunk`] items.
//! [`LlmClient::run_with_tools`] consumes the stream and emits granular
//! [`CoreEvent`]s: [`LlmStart`], [`LlmToken`], [`LlmEnd`],
//! [`ThinkingStart`], [`ThinkingToken`], [`ThinkingEnd`].
//!
//! # Thinking support
//!
//! Both Anthropic and OpenAI use encrypted opaque blobs for reasoning
//! continuity.  These blobs **must** be passed back verbatim during tool-use
//! loops within a single FSM state via [`ContentBlock::Thinking`] /
//! [`ContentBlock::RedactedThinking`].  They **never** cross FSM state
//! boundaries — the orchestrator discards [`LlmOutput::raw_content_blocks`]
//! on every state transition.

mod constants;
pub use constants::DEFAULT_MODEL;

pub mod genai;
pub mod pricing;
pub mod retry;
pub use genai::GenAiContext;

mod error;
pub use error::LlmError;

mod types;
pub use types::*;

mod images;
pub use images::{
    ImageCompletionError, ImageInput, ImageLimits, ImageMediaType, ImageRequest, ImageWire,
    ManyImages, Patches,
};

mod provider;
pub use provider::LlmProvider;

mod sse;

mod advisor;
pub use advisor::AdvisorConfig;

mod anthropic;

mod strict_schema;
pub use anthropic::AnthropicProvider;

mod openai;
pub use openai::OpenAiProvider;
pub use openai::inject_additional_properties_false;
pub use openai::validate_openai_strict_schema;

mod openai_compat;
pub use openai_compat::OpenAiCompatProvider;

mod client;
pub use client::LlmClient;

pub mod engine;
pub use engine::LlmEngine;

mod evaluator;
pub use evaluator::LlmConsistencyEvaluator;

#[cfg(test)]
mod advisor_stream_tests;
#[cfg(test)]
mod tests;
