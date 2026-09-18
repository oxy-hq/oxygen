//! Model configuration schema types for every supported LLM vendor.
//!
//! These are the parsed shape of a `model:` block in `config.yml` — the YAML
//! wire contract and the source of the generated JSON schema. They live here,
//! not in the per-vendor provider crates, so `oxy-llm` owns the config
//! vocabulary independently of the runtime provider clients (which are being
//! consolidated onto `agentic-llm`). Moved verbatim from
//! `oxy-{openai,anthropic,gemini,ollama}/src/config.rs` — field order, serde
//! attributes and derives are preserved so the schema is byte-identical.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use std::collections::HashMap;

use oxy_shared::AzureModel;

// Re-export HeaderValue so `oxy_llm::HeaderValue` keeps resolving.
pub use oxy_shared::HeaderValue;

/// Default OpenAI API URL.
pub const OPENAI_API_URL: &str = "https://api.openai.com/v1";

/// Returns the default OpenAI API URL for serde defaults.
pub fn default_openai_api_url() -> Option<String> {
    Some(OPENAI_API_URL.to_string())
}

/// Default Anthropic API URL (used for `AnthropicModelConfig`'s serde default).
pub const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1";

/// Returns the default Anthropic API URL for serde defaults.
pub fn default_anthropic_api_url() -> Option<String> {
    Some(ANTHROPIC_API_URL.to_string())
}

#[skip_serializing_none]
#[derive(Deserialize, Debug, Clone, Serialize, JsonSchema)]
pub struct OpenAIModelConfig {
    pub name: String,
    pub model_ref: String,
    pub key_var: String,
    #[serde(default = "default_openai_api_url")]
    pub api_url: Option<String>,
    #[serde(flatten)]
    pub azure: Option<AzureModel>,
    #[serde(default)]
    pub headers: Option<HashMap<String, HeaderValue>>,
}

impl OpenAIModelConfig {
    /// Get the user-defined name for this model configuration
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the underlying model name/reference used by the LLM provider
    pub fn model_name(&self) -> &str {
        &self.model_ref
    }

    /// Get the key variable name for API key resolution
    pub fn key_var(&self) -> Option<&str> {
        Some(&self.key_var)
    }

    /// Get the custom headers (if any)
    pub fn headers(&self) -> Option<&HashMap<String, HeaderValue>> {
        self.headers.as_ref()
    }
}

#[skip_serializing_none]
#[derive(Deserialize, Debug, Clone, Serialize, JsonSchema)]
pub struct AnthropicModelConfig {
    pub name: String,
    pub model_ref: String,
    pub key_var: String,
    #[serde(default = "default_anthropic_api_url")]
    pub api_url: Option<String>,
    #[serde(default)]
    pub headers: Option<HashMap<String, HeaderValue>>,
}

impl AnthropicModelConfig {
    /// Get the user-defined name for this model configuration
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the underlying model name/reference used by the LLM provider
    pub fn model_name(&self) -> &str {
        &self.model_ref
    }

    /// Get the key variable name for API key resolution
    pub fn key_var(&self) -> Option<&str> {
        Some(&self.key_var)
    }

    /// Get the custom headers (if any)
    pub fn headers(&self) -> Option<&HashMap<String, HeaderValue>> {
        self.headers.as_ref()
    }
}

#[derive(Deserialize, Debug, Clone, Serialize, JsonSchema)]
pub struct GeminiModelConfig {
    pub name: String,
    pub model_ref: String,
    pub key_var: String,
}

impl GeminiModelConfig {
    /// Get the user-defined name for this model configuration
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the underlying model name/reference used by the LLM provider
    pub fn model_name(&self) -> &str {
        &self.model_ref
    }

    /// Get the key variable name for API key resolution
    pub fn key_var(&self) -> Option<&str> {
        Some(&self.key_var)
    }
}

#[derive(Deserialize, Debug, Clone, Serialize, JsonSchema)]
pub struct OllamaModelConfig {
    pub name: String,
    pub model_ref: String,
    pub api_key: String,
    pub api_url: String,
}

impl OllamaModelConfig {
    /// Get the user-defined name for this model configuration
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the underlying model name/reference used by the LLM provider
    pub fn model_name(&self) -> &str {
        &self.model_ref
    }

    /// Get the key variable name for API key resolution (Ollama doesn't use key_var)
    pub fn key_var(&self) -> Option<&str> {
        None
    }
}

#[cfg(test)]
mod tests {
    // The config-default URLs above (what an unset `api_url` falls back to for
    // chat traffic) must stay identical to the hosts the provider crates' key-
    // validation probes hit — otherwise "Test key" would green-light a key
    // against a host that requests never actually reach. These are duplicate
    // literals only until Phase 2d deletes the provider crates and their probe
    // constants; lock them together until then.
    #[test]
    fn config_default_urls_match_provider_probe_urls() {
        assert_eq!(super::OPENAI_API_URL, oxy_openai::OPENAI_API_URL);
        assert_eq!(super::ANTHROPIC_API_URL, oxy_anthropic::ANTHROPIC_API_URL);
    }
}
