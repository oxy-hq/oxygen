use async_openai::config::OpenAIConfig;
use oxy_shared::ConfigType;

/// The Gemini API URL (OpenAI-compatible endpoint)
pub const GEMINI_API_URL: &str = "https://generativelanguage.googleapis.com/v1beta/openai";

/// Creates an OpenAI-compatible config for Google Gemini API
///
/// # Arguments
/// * `api_key` - The Google Gemini API key (resolved from secrets)
///
/// # Returns
/// A `ConfigType` configured to use Gemini's OpenAI-compatible API endpoint
pub fn create_openai_config(api_key: impl Into<String>) -> ConfigType {
    let config = OpenAIConfig::new()
        .with_api_base(GEMINI_API_URL)
        .with_api_key(api_key.into());
    ConfigType::Default(config)
}
