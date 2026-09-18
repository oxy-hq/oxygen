//! Resolve a `{ env_var: … }` header reference into a literal value.
//!
//! Small extension trait over [`HeaderValue`]: a `Direct` value passes through,
//! an `EnvVar` is looked up in the workspace secret store. Used by the agentic
//! wiring to resolve a model's `headers:` block. (Previously lived in the
//! now-removed `adapters::openai`, alongside the OpenAI client factory that
//! moved to the agentic LLM stack.)

use crate::adapters::secrets::SecretsManager;
use crate::config::model::HeaderValue;
use oxy_shared::errors::OxyError;

/// Extension trait for resolving secrets in a [`HeaderValue`].
pub trait HeaderValueExt {
    fn resolve(
        &self,
        secrets_manager: &SecretsManager,
    ) -> impl std::future::Future<Output = Result<String, OxyError>> + std::marker::Send;
}

impl HeaderValueExt for HeaderValue {
    async fn resolve(&self, secrets_manager: &SecretsManager) -> Result<String, OxyError> {
        match self {
            HeaderValue::Direct(value) => Ok(value.clone()),
            HeaderValue::EnvVar { env_var } => {
                let result = secrets_manager.resolve_secret(env_var).await?;
                match result {
                    Some(res) => Ok(res),
                    None => Err(OxyError::SecretNotFound(Some(env_var.clone()))),
                }
            }
        }
    }
}
