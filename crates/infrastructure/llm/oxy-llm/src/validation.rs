//! Unified LLM API-key validation.
//!
//! Given a provider identifier from a request body, probe whether an API key
//! would be accepted and return a structured [`KeyValidationError`] suitable
//! for surfacing to the user. Each per-vendor probe hits the vendor's `/models`
//! listing (zero token cost) with the same auth the runtime uses, then
//! interprets the HTTP status.
//!
//! The probes used to live in the `oxy-{anthropic,openai}` crates; they moved
//! here so `oxy-llm` owns key validation without depending on the provider
//! crates (the LLM runtime is `agentic-llm`).

use std::sync::OnceLock;
use std::time::Duration;

use oxy_shared::KeyValidationError;

use crate::configs::{ANTHROPIC_API_URL, OPENAI_API_URL};

/// Anthropic API version header sent with the native `/v1/models` probe.
const ANTHROPIC_API_VERSION: &str = "2023-06-01";

/// Providers we currently know how to probe for key validity.
///
/// Gemini and Ollama don't have entries because we haven't implemented
/// validation probes for them yet — `validate_provider_key` returns
/// `Unsupported` for any unknown provider so the caller can fall back to
/// "save without verifying" gracefully.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Anthropic,
    OpenAI,
}

impl ProviderKind {
    /// Parse a provider identifier as it appears in API request bodies and
    /// `vendor` fields (case-insensitive, ASCII).
    pub fn from_str_ci(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "anthropic" => Some(Self::Anthropic),
            "openai" => Some(Self::OpenAI),
            _ => None,
        }
    }
}

/// Validate an API key against the named provider. Returns `Ok(())` when the
/// provider accepts the key, or a structured error describing why the probe
/// failed.
///
/// Unknown providers (e.g. Gemini, Ollama — anything we haven't wired a probe
/// for yet) come back as `KeyValidationErrorKind::Unsupported`. That variant is
/// distinct from `Unreachable` so the user-facing message doesn't falsely blame
/// the network for a known feature gap; callers can match on it to skip
/// validation gracefully (e.g. the GitHub onboarding flow saves vendors it
/// can't probe without verification).
pub async fn validate_provider_key(
    provider: &str,
    api_key: &str,
) -> Result<(), KeyValidationError> {
    match ProviderKind::from_str_ci(provider) {
        Some(ProviderKind::Anthropic) => validate_anthropic_at(api_key, ANTHROPIC_API_URL).await,
        Some(ProviderKind::OpenAI) => validate_openai_at(api_key, OPENAI_API_URL).await,
        None => Err(KeyValidationError::unsupported(provider)),
    }
}

/// Shared `reqwest::Client` for key probes — `reqwest::Client` is internally
/// reference-counted and pools connections, so reusing one instance avoids
/// re-establishing TLS for every probe.
fn probe_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("failed to build shared LLM key-probe HTTP client")
    })
}

/// Verify an Anthropic API key by listing models against the **native** API
/// (`x-api-key` + `anthropic-version`, the same auth the agentic builder uses).
/// The explicit `base_url` lets wiremock tests exercise the full request path.
async fn validate_anthropic_at(api_key: &str, base_url: &str) -> Result<(), KeyValidationError> {
    const VENDOR: &str = "Anthropic";
    let url = format!("{base_url}/models?limit=1");
    let response = probe_client()
        .get(url)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_API_VERSION)
        .send()
        .await
        .map_err(|e| KeyValidationError::unreachable(VENDOR, e.to_string()))?;
    interpret_status(response.status(), VENDOR)
}

/// Verify an OpenAI API key by listing models with `Authorization: Bearer` —
/// the same auth scheme `async-openai` sends against chat completions.
async fn validate_openai_at(api_key: &str, base_url: &str) -> Result<(), KeyValidationError> {
    const VENDOR: &str = "OpenAI";
    let url = format!("{base_url}/models");
    let response = probe_client()
        .get(url)
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|e| KeyValidationError::unreachable(VENDOR, e.to_string()))?;
    interpret_status(response.status(), VENDOR)
}

/// Map a probe response status to the validation outcome. Pure helper so the
/// branch coverage can be unit-tested without spinning up a fake server.
fn interpret_status(status: reqwest::StatusCode, vendor: &str) -> Result<(), KeyValidationError> {
    use reqwest::StatusCode;
    if status.is_success() {
        return Ok(());
    }
    Err(match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => KeyValidationError::rejected(vendor),
        StatusCode::TOO_MANY_REQUESTS => KeyValidationError::rate_limited(vendor),
        other => KeyValidationError::unreachable(vendor, format!("HTTP {other}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy_shared::KeyValidationErrorKind;
    use reqwest::StatusCode;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn provider_kind_parses_known_names_case_insensitively() {
        assert_eq!(
            ProviderKind::from_str_ci("anthropic"),
            Some(ProviderKind::Anthropic)
        );
        assert_eq!(
            ProviderKind::from_str_ci("Anthropic"),
            Some(ProviderKind::Anthropic)
        );
        assert_eq!(
            ProviderKind::from_str_ci("  OPENAI  "),
            Some(ProviderKind::OpenAI)
        );
    }

    #[test]
    fn provider_kind_returns_none_for_unknown() {
        assert_eq!(ProviderKind::from_str_ci("gemini"), None);
        assert_eq!(ProviderKind::from_str_ci(""), None);
    }

    #[tokio::test]
    async fn validate_provider_key_marks_unknown_provider_unsupported() {
        let err = validate_provider_key("gemini", "irrelevant")
            .await
            .unwrap_err();
        assert_eq!(err.kind, KeyValidationErrorKind::Unsupported);
        // The user message must not blame reachability — Gemini works fine,
        // we just don't have a probe for it yet.
        assert!(!err.user_message().contains("could not be reached"));
        assert!(err.user_message().contains("gemini"));
    }

    #[test]
    fn interpret_status_treats_2xx_as_valid() {
        assert!(interpret_status(StatusCode::OK, "OpenAI").is_ok());
        assert!(interpret_status(StatusCode::NO_CONTENT, "OpenAI").is_ok());
    }

    #[test]
    fn interpret_status_flags_auth_failures_as_rejected() {
        assert_eq!(
            interpret_status(StatusCode::UNAUTHORIZED, "OpenAI")
                .unwrap_err()
                .kind,
            KeyValidationErrorKind::Rejected
        );
        assert_eq!(
            interpret_status(StatusCode::FORBIDDEN, "Anthropic")
                .unwrap_err()
                .kind,
            KeyValidationErrorKind::Rejected
        );
    }

    #[test]
    fn interpret_status_calls_out_rate_limits() {
        assert_eq!(
            interpret_status(StatusCode::TOO_MANY_REQUESTS, "OpenAI")
                .unwrap_err()
                .kind,
            KeyValidationErrorKind::RateLimited
        );
    }

    #[test]
    fn interpret_status_falls_back_for_other_errors() {
        let err = interpret_status(StatusCode::INTERNAL_SERVER_ERROR, "OpenAI").unwrap_err();
        match err.kind {
            KeyValidationErrorKind::Unreachable(detail) => assert!(detail.contains("500")),
            other => panic!("expected Unreachable, got {other:?}"),
        }
    }

    /// End-to-end smoke of the OpenAI probe against wiremock: asserts the URL
    /// path and `Authorization: Bearer` header (the auth `async-openai` uses).
    #[tokio::test]
    async fn openai_probe_sends_bearer_auth_on_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .and(header("authorization", "Bearer sk-test"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        assert!(validate_openai_at("sk-test", &server.uri()).await.is_ok());
    }

    #[tokio::test]
    async fn openai_probe_surfaces_rejection_on_401() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(401))
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            validate_openai_at("sk-test", &server.uri())
                .await
                .unwrap_err()
                .kind,
            KeyValidationErrorKind::Rejected
        );
    }

    /// End-to-end smoke of the Anthropic probe: asserts the `?limit=1` query,
    /// the `x-api-key` header, and the `anthropic-version` header.
    #[tokio::test]
    async fn anthropic_probe_sends_expected_request_on_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .and(query_param("limit", "1"))
            .and(header("x-api-key", "sk-test"))
            .and(header("anthropic-version", ANTHROPIC_API_VERSION))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        assert!(
            validate_anthropic_at("sk-test", &server.uri())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn anthropic_probe_surfaces_rejection_on_401() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(401))
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            validate_anthropic_at("sk-test", &server.uri())
                .await
                .unwrap_err()
                .kind,
            KeyValidationErrorKind::Rejected
        );
    }
}
