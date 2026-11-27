//! OpenAI configuration types shared across provider crates
//!
//! This module provides configuration wrapper types for OpenAI-compatible APIs,
//! allowing different providers (OpenAI, Azure, Anthropic, Gemini, Ollama) to
//! use a unified configuration interface.

use ::http::{HeaderMap, HeaderName, HeaderValue as HttpHeaderValue};
use async_openai::config::{AzureConfig, Config, OpenAIConfig};
use schemars::JsonSchema;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, str::FromStr};

/// Header value that can be either a direct string or an environment variable reference.
/// Used in model configurations to specify custom HTTP headers.
#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone)]
#[serde(untagged)]
pub enum HeaderValue {
    /// Direct header value
    Direct(String),
    /// Header value from environment variable
    EnvVar {
        /// Environment variable name containing the header value
        #[serde(rename = "env_var")]
        env_var: String,
    },
}

impl HeaderValue {
    /// Get the env var name if this is an EnvVar variant
    pub fn env_var_name(&self) -> Option<&str> {
        match self {
            HeaderValue::EnvVar { env_var } => Some(env_var),
            HeaderValue::Direct(_) => None,
        }
    }

    /// Get the direct value if this is a Direct variant
    pub fn direct_value(&self) -> Option<&str> {
        match self {
            HeaderValue::Direct(value) => Some(value),
            HeaderValue::EnvVar { .. } => None,
        }
    }
}

/// Azure OpenAI deployment configuration
#[derive(Deserialize, Debug, Clone, Serialize, JsonSchema)]
pub struct AzureModel {
    pub azure_deployment_id: String,
    pub azure_api_version: String,
}

/// Custom OpenAI configuration that supports additional headers
#[derive(Debug, Clone)]
pub struct CustomOpenAIConfig {
    base_config: OpenAIConfig,
    custom_headers: HeaderMap,
}

impl CustomOpenAIConfig {
    pub fn new(base_config: OpenAIConfig, custom_headers: HashMap<String, String>) -> Self {
        let mut header_map = HeaderMap::new();

        for (key, value) in custom_headers {
            if let (Ok(header_name), Ok(header_value)) = (
                HeaderName::from_str(&key),
                HttpHeaderValue::from_str(&value),
            ) {
                header_map.insert(header_name, header_value);
            } else {
                tracing::warn!("Invalid header: {} = {}", key, value);
            }
        }

        Self {
            base_config,
            custom_headers: header_map,
        }
    }
}

impl Config for CustomOpenAIConfig {
    fn headers(&self) -> HeaderMap {
        let mut headers = self.base_config.headers();

        for (key, value) in &self.custom_headers {
            headers.insert(key.clone(), value.clone());
        }

        headers
    }

    fn url(&self, path: &str) -> String {
        self.base_config.url(path)
    }

    fn query(&self) -> Vec<(&str, &str)> {
        self.base_config.query()
    }

    fn api_base(&self) -> &str {
        self.base_config.api_base()
    }

    fn api_key(&self) -> &SecretString {
        self.base_config.api_key()
    }
}

/// Wrapper enum for different OpenAI configuration types
#[derive(Debug, Clone)]
pub enum ConfigType {
    Default(OpenAIConfig),
    Azure(AzureConfig),
    WithHeaders(CustomOpenAIConfig),
}

/// This is a wrapper around OpenAIConfig and AzureConfig
/// to allow for dynamic configuration of the client
/// based on the model configuration
impl Config for ConfigType {
    fn headers(&self) -> HeaderMap {
        match &self {
            ConfigType::Default(config) => config.headers(),
            ConfigType::Azure(config) => config.headers(),
            ConfigType::WithHeaders(config) => config.headers(),
        }
    }
    fn url(&self, path: &str) -> String {
        match &self {
            ConfigType::Default(config) => config.url(path),
            ConfigType::Azure(config) => config.url(path),
            ConfigType::WithHeaders(config) => config.url(path),
        }
    }
    fn query(&self) -> Vec<(&str, &str)> {
        match &self {
            ConfigType::Default(config) => config.query(),
            ConfigType::Azure(config) => config.query(),
            ConfigType::WithHeaders(config) => config.query(),
        }
    }

    fn api_base(&self) -> &str {
        match &self {
            ConfigType::Default(config) => config.api_base(),
            ConfigType::Azure(config) => config.api_base(),
            ConfigType::WithHeaders(config) => config.api_base(),
        }
    }

    fn api_key(&self) -> &SecretString {
        match &self {
            ConfigType::Default(config) => config.api_key(),
            ConfigType::Azure(config) => config.api_key(),
            ConfigType::WithHeaders(config) => config.api_key(),
        }
    }
}
