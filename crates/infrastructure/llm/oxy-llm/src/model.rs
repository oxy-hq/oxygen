use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// Config schema types now live in this crate (see `configs`).
pub use crate::configs::{
    AnthropicModelConfig, GeminiModelConfig, HeaderValue, OPENAI_API_URL, OllamaModelConfig,
    OpenAIModelConfig, default_openai_api_url,
};

// Re-export from dependencies for convenience
pub use oxy_shared::AzureModel;

/// LLM model configuration supporting multiple vendors
#[derive(Deserialize, Debug, Clone, Serialize, JsonSchema)]
#[serde(tag = "vendor")]
pub enum Model {
    #[serde(rename = "openai")]
    OpenAI {
        #[serde(flatten)]
        config: OpenAIModelConfig,
    },
    #[serde(rename = "google")]
    Google {
        #[serde(flatten)]
        config: GeminiModelConfig,
    },
    #[serde(rename = "ollama")]
    Ollama {
        #[serde(flatten)]
        config: OllamaModelConfig,
    },
    #[serde(rename = "anthropic")]
    Anthropic {
        #[serde(flatten)]
        config: AnthropicModelConfig,
    },
    /// An OpenAI-compatible gateway that speaks **Chat Completions**
    /// (`/chat/completions`) rather than the Responses API (`/responses`).
    ///
    /// Distinct from [`Model::OpenAI`] only in which wire protocol the agentic
    /// pipeline targets — third-party and EU-hosted gateways (LangDock, vLLM,
    /// LM Studio, most self-hosted proxies) implement Chat Completions only and
    /// return 404 on `/responses`.
    ///
    /// Reuses [`OpenAIModelConfig`] so it inherits `key_var`, meaning the API
    /// key resolves through the managed workspace secret store like any other
    /// keyed vendor. [`Model::Ollama`] also targets Chat Completions but carries
    /// its key inline with no `key_var`, so it cannot bind a managed secret.
    #[serde(rename = "openai_compat", alias = "open_ai_compat")]
    OpenAICompat {
        #[serde(flatten)]
        config: OpenAIModelConfig,
    },
}

impl Model {
    /// Get the underlying model name/reference used by the LLM provider
    pub fn model_name(&self) -> &str {
        match self {
            Model::OpenAI { config } => config.model_name(),
            Model::Ollama { config } => config.model_name(),
            Model::Google { config } => config.model_name(),
            Model::Anthropic { config } => config.model_name(),
            Model::OpenAICompat { config } => config.model_name(),
        }
    }

    /// Get the user-defined name for this model configuration
    pub fn name(&self) -> &str {
        match self {
            Model::OpenAI { config } => config.name(),
            Model::Ollama { config } => config.name(),
            Model::Google { config } => config.name(),
            Model::Anthropic { config } => config.name(),
            Model::OpenAICompat { config } => config.name(),
        }
    }

    /// Get the key variable name for API key resolution (if applicable)
    pub fn key_var(&self) -> Option<&str> {
        match self {
            Model::OpenAI { config } => config.key_var(),
            Model::Google { config } => config.key_var(),
            Model::Anthropic { config } => config.key_var(),
            Model::Ollama { config } => config.key_var(), // Returns None
            Model::OpenAICompat { config } => config.key_var(),
        }
    }

    /// Get the custom headers for models that support them (if any)
    pub fn headers(&self) -> Option<&HashMap<String, HeaderValue>> {
        match self {
            Model::OpenAI { config } => config.headers(),
            Model::Anthropic { config } => config.headers(),
            Model::OpenAICompat { config } => config.headers(),
            _ => None,
        }
    }

    /// Get inner OpenAI config if this is an OpenAI model
    pub fn as_openai(&self) -> Option<&OpenAIModelConfig> {
        match self {
            Model::OpenAI { config } => Some(config),
            _ => None,
        }
    }

    /// Get inner Anthropic config if this is an Anthropic model
    pub fn as_anthropic(&self) -> Option<&AnthropicModelConfig> {
        match self {
            Model::Anthropic { config } => Some(config),
            _ => None,
        }
    }

    /// Get inner Google/Gemini config if this is a Google model
    pub fn as_google(&self) -> Option<&GeminiModelConfig> {
        match self {
            Model::Google { config } => Some(config),
            _ => None,
        }
    }

    /// Get inner Ollama config if this is an Ollama model
    pub fn as_ollama(&self) -> Option<&OllamaModelConfig> {
        match self {
            Model::Ollama { config } => Some(config),
            _ => None,
        }
    }
}
