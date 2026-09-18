//! LLM model configuration types for Oxy
//!
//! This crate owns the model config schema types for every supported LLM vendor
//! — [`OpenAIModelConfig`], [`AnthropicModelConfig`], [`GeminiModelConfig`],
//! [`OllamaModelConfig`] (the `model:` YAML wire contract) — plus the unified
//! [`Model`] enum that composes them. Runtime provider clients live in the
//! per-vendor crates (being consolidated onto `agentic-llm`); validation and
//! secret resolution are handled separately in the core crate.

mod configs;
mod model;
mod traits;
mod validation;

// Re-export the unified Model enum and all provider config types
pub use model::{
    AnthropicModelConfig, AzureModel, GeminiModelConfig, HeaderValue, Model, OPENAI_API_URL,
    OllamaModelConfig, OpenAIModelConfig, default_openai_api_url,
};

pub use traits::ModelConfig;

// Default Anthropic API URL function (kept as a public entry point).
pub use configs::default_anthropic_api_url;

// API-key validation entry points. Provider-specific probes live in their
// respective crates; `validate_provider_key` dispatches by name so HTTP
// handlers (and any future Settings "Test key" affordances) call one
// function instead of matching providers themselves.
pub use oxy_shared::{KeyValidationError, KeyValidationErrorKind};
pub use validation::{ProviderKind, validate_provider_key};
