//! One model call, and the retry policy around it.

use std::time::Duration;

use agentic_core::react::{EngineError, EventSink};
use agentic_core::tools::ToolDef;
use serde_json::Value;
use tracing::Instrument as _;

use super::LlmEngine;
use super::fold::{self, Folded, STATE};
use crate::LlmError;
use crate::genai::{self, InferenceRequest};
use crate::retry::{rate_limit_backoff_secs, transient_backoff_secs};

/// The request for one provider stream.
pub(super) struct Attempt<'a> {
    pub system: &'a str,
    pub messages: &'a [Value],
    pub tools: &'a [ToolDef],
    pub max_tokens: Option<u32>,
    /// Streams opened so far in this `decide` (continuations and re-streams).
    pub round: u32,
}

/// Retries spent in one `decide`; the two budgets are independent.
#[derive(Default)]
pub(super) struct RetryBudget {
    rate_limit: u32,
    transient: u32,
}

impl LlmEngine {
    /// One stream, retried on rate limits and transient errors. A retry
    /// re-streams from scratch, so tokens of a failed attempt may already be
    /// on the event stream, but only the successful attempt is returned.
    pub(super) async fn call_with_retry(
        &self,
        a: &Attempt<'_>,
        budget: &mut RetryBudget,
        ev: &EventSink,
    ) -> Result<Folded, EngineError> {
        loop {
            let err = match self.call(a, ev).await {
                Ok(f) => return Ok(f),
                Err(e) => e,
            };
            let wait = self.backoff(err, budget)?;
            tokio::time::sleep(wait).await;
        }
    }

    /// The wait before the next attempt, or the error to surface when the
    /// failure is not retryable or its budget is spent.
    fn backoff(&self, err: LlmError, budget: &mut RetryBudget) -> Result<Duration, EngineError> {
        match err {
            LlmError::RateLimit {
                message,
                retry_after,
            } => {
                if budget.rate_limit >= self.cfg.max_rate_limit_retries {
                    return Err(EngineError::RateLimited {
                        retry_after: retry_after.map(|d| d.as_secs()),
                    });
                }
                let wait = retry_after.unwrap_or_else(|| {
                    Duration::from_secs(rate_limit_backoff_secs(budget.rate_limit))
                });
                budget.rate_limit += 1;
                tracing::warn!(attempt = budget.rate_limit, ?wait, %message, "react engine: rate limited; retrying");
                Ok(wait)
            }
            LlmError::Transient(message) => {
                if budget.transient >= self.cfg.max_transient_retries {
                    return Err(EngineError::Transient(message));
                }
                let wait = Duration::from_secs_f64(transient_backoff_secs(budget.transient));
                budget.transient += 1;
                tracing::warn!(attempt = budget.transient, ?wait, %message, "react engine: transient error; retrying");
                Ok(wait)
            }
            other => Err(EngineError::Fatal(other.to_string())),
        }
    }

    /// Opens one observed stream inside its inference span and folds it.
    async fn call(&self, a: &Attempt<'_>, ev: &EventSink) -> Result<Folded, LlmError> {
        let span = genai::inference_span(
            self.client.provider(),
            self.client.genai_context(),
            &InferenceRequest {
                system: a.system,
                messages: a.messages,
                max_tokens: a.max_tokens,
                tool_count: a.tools.len(),
                structured_output: false,
                state: Some(STATE),
                round: Some(a.round),
            },
        );
        async {
            let stream = self
                .client
                .observed_stream(
                    &span,
                    a.system,
                    "",
                    a.messages,
                    a.tools,
                    &self.cfg.thinking,
                    None,
                    a.max_tokens,
                )
                .await?;
            fold::fold(stream, ev).await
        }
        .instrument(span.clone())
        .await
    }
}
