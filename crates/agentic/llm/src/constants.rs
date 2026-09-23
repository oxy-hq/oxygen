use std::time::Duration;

use crate::LlmError;

/// Root of the Anthropic API — the `/messages` path is appended by
/// `AnthropicProvider::messages_url`, mirroring [`OPENAI_BASE_URL`].
///
/// Deliberately named `…_BASE_URL`, not `ANTHROPIC_API_URL`: a same-named
/// constant (`ANTHROPIC_API_URL`, a bare root) lives in `oxy-llm`, and while
/// this one held a full `/v1/messages` URL and that one held a root, the two
/// were trivially confusable — which is how a config-supplied root once reached
/// a provider that POSTed it verbatim.
pub(super) const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com/v1";
pub(super) const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Beta feature flag required for extended thinking / streaming thinking.
/// Without this header, the API silently ignores the `thinking` body
/// parameter and no thinking blocks appear in the SSE stream.
pub(super) const ANTHROPIC_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";
pub(super) const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

/// The default model used when constructing an [`LlmClient`] via [`LlmClient::new`].
pub const DEFAULT_MODEL: &str = "claude-opus-4-8";

pub(super) const DEFAULT_MAX_TOKENS: u32 = 4096;
/// Higher token cap used when extended thinking is enabled.  Thinking
/// output (especially Manual budgets) can consume thousands of tokens;
/// the text response needs room on top of that.
pub(super) const THINKING_MAX_TOKENS: u32 = 16384;

/// Connect timeout for LLM provider HTTP clients.  Bounds only TCP/TLS
/// connection establishment — **not** the (potentially long) streaming body.
pub(super) const LLM_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Idle read timeout for streaming LLM responses.  reqwest resets this on every
/// chunk read, so it bounds the gap *between* bytes rather than the total
/// stream duration: a legitimately long generation is fine as long as tokens
/// keep arriving, while a silently dead connection is cut instead of hanging
/// forever.  A hard overall `.timeout()` is deliberately avoided because it
/// would truncate long legitimate streams.
pub(super) const LLM_READ_TIMEOUT: Duration = Duration::from_secs(120);

/// Build a reqwest client for LLM providers with connect + idle-read timeouts.
///
/// Falls back to a default (timeout-less) client if the builder fails, which is
/// unreachable in practice for these settings — the fallback only exists so the
/// infallible `Provider::new` constructors keep their signatures.
pub(super) fn build_llm_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(LLM_CONNECT_TIMEOUT)
        .read_timeout(LLM_READ_TIMEOUT)
        .build()
        .unwrap_or_else(|err| {
            tracing::warn!(
                error = %err,
                "failed to build LLM HTTP client with timeouts; using default client"
            );
            reqwest::Client::new()
        })
}

/// Parse a `Retry-After` response header into a [`Duration`].
///
/// Only the delta-seconds form (`Retry-After: 30`) is honored — the HTTP-date
/// form is rare for 429s and, when absent or unparseable, the caller falls back
/// to its computed exponential backoff, so returning `None` is always safe.
pub(super) fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let raw = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    let secs: u64 = raw.trim().parse().ok()?;
    Some(Duration::from_secs(secs))
}

/// Classify a reqwest transport error from `.send()` into an [`LlmError`].
///
/// Connection failures and timeouts are transient — worth a bounded retry —
/// and map to [`LlmError::Transient`].  Everything else surfaces as
/// [`LlmError::Http`] (non-retryable).
pub(super) fn send_error_to_llm(err: reqwest::Error) -> LlmError {
    if err.is_connect() || err.is_timeout() {
        LlmError::Transient(err.to_string())
    } else {
        LlmError::Http(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers_with_retry_after(value: &str) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(
            reqwest::header::RETRY_AFTER,
            reqwest::header::HeaderValue::from_str(value).unwrap(),
        );
        h
    }

    #[test]
    fn parses_delta_seconds() {
        let h = headers_with_retry_after("30");
        assert_eq!(parse_retry_after(&h), Some(Duration::from_secs(30)));
    }

    #[test]
    fn ignores_http_date_form() {
        // The rare HTTP-date form is not honored — falls back to computed backoff.
        let h = headers_with_retry_after("Wed, 21 Oct 2026 07:28:00 GMT");
        assert_eq!(parse_retry_after(&h), None);
    }

    #[test]
    fn none_when_header_absent() {
        let h = reqwest::header::HeaderMap::new();
        assert_eq!(parse_retry_after(&h), None);
    }
}
