use std::collections::HashSet;
use std::sync::Arc;

use uuid::Uuid;

use crate::{
    adapters::secrets::{
        SecretsDatabaseStorage, SecretsStorage,
        environment::SecretsEnvironmentStorage,
        storage::{SecretsFallbackStorage, SecretsStorageImpl},
    },
    service::secret_manager::SecretManagerService,
};
use oxy_shared::errors::OxyError;

#[derive(Debug, Clone)]
pub struct SecretsManager {
    storage: SecretsStorageImpl,
    /// Names this manager must never resolve or write — see [`Self::withholding`].
    withheld: Option<Arc<HashSet<String>>>,
}

impl SecretsManager {
    pub fn from_environment() -> Result<Self, OxyError> {
        Ok(SecretsManager {
            storage: SecretsStorageImpl::EnvironmentStorage(SecretsEnvironmentStorage {}),
            withheld: None,
        })
    }

    pub fn from_database(secret_manager: SecretManagerService) -> Result<Self, OxyError> {
        let secrets_database_storage = SecretsDatabaseStorage::new(secret_manager);
        Ok(SecretsManager {
            storage: SecretsStorageImpl::DatabaseStorage(secrets_database_storage),
            withheld: None,
        })
    }

    /// DB-first with env fallback. Used in local mode so that a DB secret
    /// immediately overrides the matching env var without a server restart.
    pub fn from_database_with_env_fallback(
        secret_manager: SecretManagerService,
    ) -> Result<Self, OxyError> {
        let db_storage = SecretsDatabaseStorage::new(secret_manager);
        Ok(SecretsManager {
            storage: SecretsStorageImpl::FallbackStorage(SecretsFallbackStorage::new(db_storage)),
            withheld: None,
        })
    }

    /// This manager, except that every name in `names` resolves to nothing and
    /// cannot be written. For a context that runs a workspace's code but must
    /// not reach one of its credentials — a workspace preview holding
    /// production's rotate-on-use tokens. Every read in the workspace (a
    /// database `*_var`, an LLM `key_var`, a header `env_var`) goes through
    /// [`Self::resolve_secret`], so one gate here covers them; a caller with its
    /// own environment fallback asks [`Self::withholds`] first.
    pub fn withholding(mut self, names: HashSet<String>) -> Self {
        self.withheld = Some(Arc::new(names));
        self
    }

    /// Whether `secret_name` is withheld from this manager.
    pub fn withholds(&self, secret_name: &str) -> bool {
        self.withheld
            .as_ref()
            .is_some_and(|w| w.contains(secret_name))
    }

    fn refuse_withheld(&self, secret_name: &str) -> Result<(), OxyError> {
        if self.withholds(secret_name) {
            return Err(OxyError::SecretManager(format!(
                "`{secret_name}` is withheld from this context"
            )));
        }
        Ok(())
    }

    pub async fn resolve_secret(&self, secret_name: &str) -> Result<Option<String>, OxyError> {
        if self.withholds(secret_name) {
            return Ok(None);
        }
        self.storage.resolve_secret(secret_name).await
    }

    pub async fn create_secret(
        &self,
        secret_name: &str,
        secret_value: &str,
        created_by: Uuid,
    ) -> Result<(), OxyError> {
        self.refuse_withheld(secret_name)?;
        self.storage
            .create_secret(secret_name, secret_value, created_by)
            .await
    }

    /// Create the secret, or atomically overwrite its value if it exists.
    /// Prefer this over `remove_secret` + `create_secret` for rotated
    /// credentials — there is no window in which the value is absent.
    pub async fn upsert_secret(
        &self,
        secret_name: &str,
        secret_value: &str,
        updated_by: Uuid,
    ) -> Result<(), OxyError> {
        self.refuse_withheld(secret_name)?;
        self.storage
            .upsert_secret(secret_name, secret_value, updated_by)
            .await
    }

    pub async fn remove_secret(&self, secret_name: &str) -> Result<(), OxyError> {
        self.refuse_withheld(secret_name)?;
        self.storage.remove_secret(secret_name).await
    }

    /// Resolve a config value from either a direct value or an environment variable.
    ///
    /// This is a common pattern used throughout the codebase for config fields that
    /// can be specified directly or via an environment variable reference.
    ///
    /// # Arguments
    /// * `direct_value` - The direct value if specified (e.g., `password` field)
    /// * `var_name` - The environment variable name if specified (e.g., `password_var` field)
    /// * `field_name` - Human-readable field name for error messages
    /// * `default` - Optional default value if neither direct nor var is specified
    ///
    /// # Returns
    /// * `Ok(String)` - The resolved value
    /// * `Err(OxyError::SecretNotFound)` - If var_name was specified but the secret wasn't found
    /// * `Err(OxyError::ConfigurationError)` - If no value could be resolved and no default provided
    pub async fn resolve_config_value(
        &self,
        direct_value: Option<&str>,
        var_name: Option<&str>,
        field_name: &str,
        default: Option<&str>,
    ) -> Result<String, OxyError> {
        // Try direct value first
        if let Some(value) = direct_value
            && !value.is_empty()
        {
            return Ok(value.to_string());
        }

        // Try resolving from environment variable
        if let Some(var) = var_name
            && !var.is_empty()
        {
            let resolved = self.resolve_secret(var).await?;
            if let Some(res) = resolved {
                return Ok(res);
            }
            return Err(OxyError::SecretNotFound(Some(var.to_string())));
        }

        // Fall back to default if provided
        if let Some(def) = default {
            return Ok(def.to_string());
        }

        // No value found
        Err(OxyError::ConfigurationError(format!(
            "{} or {}_var must be specified",
            field_name, field_name
        )))
    }
}

#[cfg(test)]
mod withholding_tests {
    use super::*;

    /// A withheld name resolves to nothing — directly and through
    /// `resolve_config_value` (a database `password_var`) — and cannot be
    /// written; every other name is untouched.
    #[tokio::test]
    async fn a_withheld_name_neither_resolves_nor_writes() {
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::set_var("S9_WITHHELD_TOKEN", "secret");
            std::env::set_var("S9_ORDINARY_TOKEN", "fine");
        }
        let sm = SecretsManager::from_environment()
            .unwrap()
            .withholding(HashSet::from(["S9_WITHHELD_TOKEN".to_string()]));
        assert!(sm.withholds("S9_WITHHELD_TOKEN"));
        assert_eq!(sm.resolve_secret("S9_WITHHELD_TOKEN").await.unwrap(), None);
        assert!(
            sm.resolve_config_value(None, Some("S9_WITHHELD_TOKEN"), "password", None)
                .await
                .is_err()
        );
        assert!(
            sm.upsert_secret("S9_WITHHELD_TOKEN", "x", Uuid::nil())
                .await
                .is_err()
        );
        assert_eq!(
            sm.resolve_secret("S9_ORDINARY_TOKEN")
                .await
                .unwrap()
                .as_deref(),
            Some("fine"),
            "the control resolves"
        );
    }
}
