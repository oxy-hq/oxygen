use std::collections::HashSet;
use std::sync::Arc;

use uuid::Uuid;

use crate::{
    adapters::secrets::SecretsManager,
    config::{ConfigManager, ReadOnly, WorkingCopy},
    intent::IntentClassifier,
};
use oxy_shared::errors::OxyError;

/// `S` is the filesystem capability of the process holding this manager —
/// [`crate::config::WorkingCopy`] on the ide / a single-process instance,
/// [`crate::config::ReadOnly`] on a stateless serve replica. It rides on the
/// contained [`ConfigManager`], so a helper that needs the working copy must
/// ask for `WorkspaceManager<WorkingCopy>` and the requirement propagates up the call
/// chain on its own.
#[derive(Debug, Clone)]
pub struct WorkspaceManager<S> {
    pub workspace_id: Uuid,
    pub config_manager: ConfigManager<S>,
    pub secrets_manager: SecretsManager,
    pub intent_classifier: Option<Arc<IntentClassifier>>,
}

impl<S> WorkspaceManager<S> {
    /// The inverse of [`Self::into_read_only`], for a caller that has checked
    /// this node owns the files and needs a method that requires them.
    ///
    /// Takes the escalated `ConfigManager` rather than re-deriving it, so the
    /// check and the use cannot drift: the only way to get one is
    /// `ResolveWorkspaceFile::workspace_file_resolver`, which returns `None`
    /// when there is nothing to escalate to.
    pub fn into_working_copy(
        self,
        config_manager: ConfigManager<WorkingCopy>,
    ) -> WorkspaceManager<WorkingCopy> {
        WorkspaceManager {
            workspace_id: self.workspace_id,
            config_manager,
            secrets_manager: self.secrets_manager,
            intent_classifier: self.intent_classifier,
        }
    }

    pub(super) fn new(
        workspace_id: Uuid,
        config_manager: ConfigManager<S>,
        secrets_manager: SecretsManager,
        intent_classifier: Option<Arc<IntentClassifier>>,
    ) -> Self {
        Self {
            workspace_id,
            config_manager,
            secrets_manager,
            intent_classifier,
        }
    }

    pub async fn get_required_secrets(&self) -> Result<Option<Vec<String>>, OxyError> {
        let mut secrets_to_check: HashSet<String> = HashSet::new();

        let config_manager = &self.config_manager;

        let config = config_manager.get_config();

        for model in &config.models {
            if let Some(key_var) = config_manager.get_model_key_var(model) {
                let secret = self.secrets_manager.resolve_secret(&key_var).await?;
                tracing::info!(
                    "Checking model key variable: {}, value: {:?}",
                    key_var,
                    secret.clone()
                );
                // Only add to secrets_to_check if it's not already resolvable
                if secret.is_none() {
                    secrets_to_check.insert(key_var);
                }
            }
        }

        // Check database configurations for password_var requirements
        for database in &config.databases {
            if let Some(password_var) = config_manager.get_database_password_var(database) {
                tracing::info!("Checking database password variable: {}", password_var);
                // Only add to secrets_to_check if it's not already resolvable
                if self
                    .secrets_manager
                    .resolve_secret(&password_var)
                    .await?
                    .is_none()
                {
                    secrets_to_check.insert(password_var);
                }
            }
        }

        if secrets_to_check.is_empty() {
            Ok(None)
        } else {
            Ok(Some(secrets_to_check.into_iter().collect()))
        }
    }
}

impl WorkspaceManager<WorkingCopy> {
    /// Give up the filesystem capability — see
    /// [`ConfigManager::without_working_copy`].
    /// Keep the working copy, drop the requirement. See
    /// [`ConfigManager::into_read_only`].
    pub fn into_read_only(self) -> WorkspaceManager<ReadOnly> {
        WorkspaceManager {
            workspace_id: self.workspace_id,
            config_manager: self.config_manager.into_read_only(),
            secrets_manager: self.secrets_manager,
            intent_classifier: self.intent_classifier,
        }
    }

    pub fn without_working_copy(self) -> WorkspaceManager<ReadOnly> {
        WorkspaceManager {
            workspace_id: self.workspace_id,
            config_manager: self.config_manager.without_working_copy(),
            secrets_manager: self.secrets_manager,
            intent_classifier: self.intent_classifier,
        }
    }
}
