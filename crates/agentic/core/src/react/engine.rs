//! The engine contract: one model call per [`Engine::decide`].

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::transcript::Transcript;
use crate::tools::ToolDef;

/// Raw `(event_type, payload)` channel, the shape `ExecutingTask.events`
/// carries, so a worker can forward it unchanged.
pub type EventSink = tokio::sync::mpsc::Sender<(String, Value)>;

/// A tool call the model asked for.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolCallReq {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// Token and cost accounting for one model call.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct TurnUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_usd: Option<f64>,
    pub model: String,
}

/// One assistant turn.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Turn {
    pub text: String,
    pub tool_calls: Vec<ToolCallReq>,
    /// Provider-native content blocks (thinking blocks and their signatures
    /// included), replayed verbatim on the next call.
    pub native: Value,
    pub usage: TurnUsage,
}

/// Runs when the loop reaches `Done`, before the `done` row is written, so
/// rows it emits precede `done`. It may rewrite the answer turn (its text).
#[async_trait::async_trait]
pub trait DoneHook: Send + Sync {
    async fn on_done(&self, answer: &mut Turn, ev: &EventSink);
}

/// What the model decided: answer, or call tools and continue.
#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    Reply(Turn),
    CallTools(Turn),
}

#[derive(thiserror::Error, Debug)]
pub enum EngineError {
    #[error("rate limited (retry after {retry_after:?}s)")]
    RateLimited { retry_after: Option<u64> },
    #[error("transient: {0}")]
    Transient(String),
    #[error("fatal: {0}")]
    Fatal(String),
    #[error("max tokens reached")]
    MaxTokens { partial_text: String },
}

impl EngineError {
    /// Server-suggested wait for a rate limit, if any.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimited {
                retry_after: Some(s),
            } => Some(Duration::from_secs(*s)),
            _ => None,
        }
    }
}

#[async_trait]
pub trait Engine: Send + Sync {
    async fn decide(
        &self,
        transcript: &Transcript,
        tools: &[ToolDef],
        events: &EventSink,
    ) -> Result<Decision, EngineError>;
}
