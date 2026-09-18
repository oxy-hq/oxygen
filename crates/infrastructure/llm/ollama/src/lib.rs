use async_openai::config::OpenAIConfig;
use oxy_shared::ConfigType;

/// The default Ollama API URL
pub const DEFAULT_OLLAMA_API_URL: &str = "http://localhost:11434/v1";

/// Creates an OpenAI-compatible config for Ollama API
///
/// # Arguments
/// * `api_key` - The Ollama API key (can be any string, Ollama typically doesn't require authentication)
/// * `api_url` - The Ollama API URL (typically http://localhost:11434/v1 or custom endpoint)
///
/// # Returns
/// A `ConfigType` configured to use Ollama's API endpoint
pub fn create_openai_config(api_key: impl Into<String>, api_url: impl Into<String>) -> ConfigType {
    let config = OpenAIConfig::new()
        .with_api_base(api_url.into())
        .with_api_key(api_key.into());
    ConfigType::Default(config)
}
