use crate::models::{
    AnthropicModelConfig, GoogleModelConfig, ModelConfig, ModelVendor, ModelsFormData,
    OllamaModelConfig, OpenAIModelConfig,
};
use axum::http::StatusCode;
use oxy::config::constants::{
    ANTHROPIC_API_KEY_VAR, DEFAULT_ANTHROPIC_MODEL, DEFAULT_GOOGLE_MODEL, DEFAULT_OPENAI_MODEL,
    OPENAI_API_KEY_VAR,
};
use oxy::config::model::{
    AnthropicModelConfig as LlmAnthropicConfig, GeminiModelConfig as LlmGeminiConfig, Model,
    OllamaModelConfig as LlmOllamaConfig, OpenAIModelConfig as LlmOpenAIConfig,
};
use oxy::service::secret_manager::{CreateSecretParams, SecretManagerService};
use oxy_shared::errors::OxyError;
use sea_orm::DatabaseTransaction;
use tracing::error;
use uuid::Uuid;

pub struct ModelConfigBuilder;

impl ModelConfigBuilder {
    pub async fn build_model_configs(
        project_id: Uuid,
        user_id: Uuid,
        models_form: &ModelsFormData,
        txn: &DatabaseTransaction,
    ) -> std::result::Result<Vec<Model>, StatusCode> {
        let mut config_models = Vec::new();

        for model_config in &models_form.models {
            let model = match &model_config.vendor {
                ModelVendor::OpenAI => {
                    Self::build_openai_model(project_id, user_id, model_config, txn).await?
                }
                ModelVendor::Anthropic => {
                    Self::build_anthropic_model(project_id, user_id, model_config, txn).await?
                }
                ModelVendor::Google => {
                    Self::build_google_model(project_id, user_id, model_config, txn).await?
                }
                ModelVendor::Ollama => Self::build_ollama_model(model_config),
            };

            config_models.push(model);
        }

        Ok(config_models)
    }

    async fn build_openai_model(
        project_id: Uuid,
        user_id: Uuid,
        model_config: &ModelConfig,
        txn: &DatabaseTransaction,
    ) -> std::result::Result<Model, StatusCode> {
        let openai_config: OpenAIModelConfig = model_config.get_openai_config();

        let name = model_config
            .name
            .clone()
            .unwrap_or_else(|| "openai-model".to_string());
        let model_ref = openai_config
            .model_ref
            .unwrap_or_else(|| DEFAULT_OPENAI_MODEL.to_string());
        let key_var = name.to_uppercase() + "_API_KEY";

        Self::create_secret(
            project_id,
            user_id,
            key_var.clone(),
            openai_config
                .api_key
                .unwrap_or_else(|| OPENAI_API_KEY_VAR.to_string()),
            txn,
        )
        .await
        .map_err(|e| {
            error!("Failed to create OpenAI API key secret: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        Ok(Model::OpenAI {
            config: LlmOpenAIConfig {
                name,
                model_ref,
                key_var,
                api_url: openai_config.api_url.filter(|url| !url.is_empty()),
                azure: None,
                headers: None,
            },
        })
    }

    async fn build_anthropic_model(
        project_id: Uuid,
        user_id: Uuid,
        model_config: &ModelConfig,
        txn: &DatabaseTransaction,
    ) -> std::result::Result<Model, StatusCode> {
        let anthropic_config: AnthropicModelConfig = model_config.get_anthropic_config();

        let name = model_config
            .name
            .clone()
            .unwrap_or_else(|| "anthropic-model".to_string());
        // A model added without an explicit `model_ref` must not silently fall
        // back to a retiring id. `claude-3-opus-20240229` (Claude 3) is on
        // Anthropic's deprecation path and may 404 once sunset; default to the
        // current platform default instead. The canonical value lives in
        // `oxy::config::constants::DEFAULT_ANTHROPIC_MODEL`, kept in sync with
        // `agentic_llm::constants::DEFAULT_MODEL`.
        let model_ref = anthropic_config
            .model_ref
            .unwrap_or_else(|| DEFAULT_ANTHROPIC_MODEL.to_string());
        let key_var = name.to_uppercase() + "_API_KEY";

        Self::create_secret(
            project_id,
            user_id,
            key_var.clone(),
            anthropic_config
                .api_key
                .unwrap_or_else(|| ANTHROPIC_API_KEY_VAR.to_string()),
            txn,
        )
        .await
        .map_err(|e| {
            error!("Failed to create Anthropic API key secret: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        Ok(Model::Anthropic {
            config: LlmAnthropicConfig {
                name,
                model_ref,
                key_var,
                api_url: anthropic_config.api_url.filter(|url| !url.is_empty()),
                headers: None,
            },
        })
    }

    async fn build_google_model(
        project_id: Uuid,
        user_id: Uuid,
        model_config: &ModelConfig,
        txn: &DatabaseTransaction,
    ) -> std::result::Result<Model, StatusCode> {
        let google_config: GoogleModelConfig = model_config.get_google_config();

        let name = model_config
            .name
            .clone()
            .unwrap_or_else(|| "google-model".to_string());
        let model_ref = google_config
            .model_ref
            .unwrap_or_else(|| DEFAULT_GOOGLE_MODEL.to_string());
        let key_var = name.to_uppercase() + "_API_KEY";

        Self::create_secret(
            project_id,
            user_id,
            key_var.clone(),
            google_config
                .api_key
                .unwrap_or_else(|| "GOOGLE_API_KEY".to_string()),
            txn,
        )
        .await
        .map_err(|e| {
            error!("Failed to create Google API key secret: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        Ok(Model::Google {
            config: LlmGeminiConfig {
                name,
                model_ref,
                key_var,
            },
        })
    }

    fn build_ollama_model(model_config: &ModelConfig) -> Model {
        let ollama_config: OllamaModelConfig = model_config.get_ollama_config();

        let name = model_config
            .name
            .clone()
            .unwrap_or_else(|| "ollama-model".to_string());
        let model_ref = ollama_config
            .model_ref
            .unwrap_or_else(|| "llama2".to_string());
        let api_url = ollama_config
            .api_url
            .unwrap_or_else(|| "http://localhost:11434/api".to_string());
        let api_key = ollama_config
            .api_key
            .unwrap_or_else(|| "api_key".to_string());

        Model::Ollama {
            config: LlmOllamaConfig {
                name,
                model_ref,
                api_key,
                api_url,
            },
        }
    }

    async fn create_secret(
        project_id: Uuid,
        user_id: Uuid,
        key: String,
        value: String,
        txn: &DatabaseTransaction,
    ) -> Result<(), OxyError> {
        let secret_manager = SecretManagerService::new(project_id);
        let create_params = CreateSecretParams {
            name: key,
            value,
            description: None,
            created_by: user_id,
        };

        secret_manager.create_secret(txn, create_params).await?;
        Ok(())
    }
}
