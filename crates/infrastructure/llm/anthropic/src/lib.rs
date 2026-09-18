//! Anthropic LLM provider implementation
//!
//! # Implementation approach
//!
//! This crate uses `async-openai` pointed at Anthropic's **OpenAI-compatible endpoint**
//! (`https://api.anthropic.com/v1`). This covers our current production needs:
//! chat completions, streaming, tool calling, and custom headers.
//!
//! # Feature parity with OpenAI adapter
//!
//! | Feature              | Status |
//! |----------------------|--------|
//! | Chat completions     | ✅     |
//! | Streaming            | ✅     |
//! | Tool use             | ✅     |
//! | Custom headers       | ✅     |
//! | Custom API URL       | ✅     |
//! | Vision (images)      | ✅ (partial — `detail` parameter ignored) |
//!
//! # Known limitations of the OpenAI-compatible endpoint
//!
//! The following Anthropic-native features are **not available** through the compat endpoint.
//! Anthropic's own docs state it is "not considered a long-term or production-ready solution
//! for most use cases". Custom headers (added in this crate) provide a workaround for some
//! beta features via `anthropic-beta` header, but the request/response shapes still differ.
//!
//! | Missing feature        | Why it matters                                               | Workaround |
//! |------------------------|--------------------------------------------------------------|------------|
//! | **Extended thinking**  | Full reasoning trace not returned via compat endpoint        | Pass `anthropic-beta` header + `extra_body`, but reasoning output is stripped |
//! | **Prompt caching**     | Can cut costs 90% on repeated context; requires `cache_control` on message blocks | None via compat |
//! | **PDF / document blocks** | Native multi-page document analysis                    | None via compat |
//! | **Batch API**          | Async bulk requests at 50% discount                         | None via compat |
//! | **Computer use** (beta)| Desktop automation tools                                    | None via compat |
//! | **Strict tool schemas** | `strict` parameter ignored; schema conformance not guaranteed | None via compat |
//! | **Token counting endpoint** | Pre-flight cost estimation                           | None via compat |
//!
//! # Migration path (when needed)
//!
//! No **official** Anthropic Rust SDK exists as of 2026-02. The top unofficial options are:
//!
//! - [`async-anthropic`](https://crates.io/crates/async-anthropic) — most downloaded (25k),
//!   but lacks extended thinking and prompt caching
//! - [`misanthropic`](https://crates.io/crates/misanthropic) — has prompt caching, but
//!   low adoption (6 stars, last commit Dec 2024)
//! - [`anthropic-sdk-rust`](https://crates.io/crates/anthropic-sdk-rust) — claims TypeScript
//!   SDK parity but low adoption (7 stars)
//!
//! **Recommendation:** migrate when either (a) an official Rust SDK ships, or (b) extended
//! thinking or prompt caching becomes a product requirement and a mature SDK supports it.
//! All Anthropic-specific logic is isolated in this crate, so migration is contained.

mod validation;

use async_openai::config::OpenAIConfig;
use oxy_shared::{ConfigType, CustomOpenAIConfig};
use std::collections::HashMap;

pub use validation::validate_api_key;

/// The default Anthropic API URL
pub const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1";

/// Vendor label used in user-facing messages (e.g. validation errors).
pub const VENDOR_LABEL: &str = "Anthropic";

/// Anthropic API version header value sent with native-protocol requests
/// (e.g. the `/v1/models` listing used by `validate_api_key`).
pub const ANTHROPIC_API_VERSION: &str = "2023-06-01";

/// Creates an OpenAI-compatible config for Anthropic API
///
/// # Arguments
/// * `api_key` - The Anthropic API key (already resolved from secrets)
/// * `api_url` - Optional custom API URL (defaults to ANTHROPIC_API_URL)
/// * `custom_headers` - Optional map of resolved custom HTTP headers
///
/// # Returns
/// A `ConfigType` configured to use Anthropic's API endpoint
pub fn create_openai_config(
    api_key: impl Into<String>,
    api_url: Option<String>,
    custom_headers: Option<HashMap<String, String>>,
) -> ConfigType {
    let config = OpenAIConfig::new()
        .with_api_base(api_url.unwrap_or_else(|| ANTHROPIC_API_URL.to_string()))
        .with_api_key(api_key.into());

    if let Some(headers) = custom_headers {
        let config_with_headers = CustomOpenAIConfig::new(config, headers);
        ConfigType::WithHeaders(config_with_headers)
    } else {
        ConfigType::Default(config)
    }
}
