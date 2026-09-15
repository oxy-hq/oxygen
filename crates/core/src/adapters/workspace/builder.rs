use std::path::Path;
use std::sync::Arc;

use crate::{
    adapters::{secrets::SecretsManager, workspace::manager::WorkspaceManager},
    config::{ConfigBuilder, ConfigManager, OnMissing, Origin, ReadOnly, WorkingCopy},
    intent::{IntentClassifier, IntentConfig},
};
use oxy_shared::errors::OxyError;

pub struct WorkspaceBuilder<S> {
    workspace_id: Option<uuid::Uuid>,
    config_manager: Option<ConfigManager<S>>,
    secrets_manager: Option<SecretsManager>,
    intent_classifier: Option<Arc<IntentClassifier>>,
}

impl<S> Default for WorkspaceBuilder<S> {
    fn default() -> Self {
        Self {
            workspace_id: None,
            config_manager: None,
            secrets_manager: None,
            intent_classifier: None,
        }
    }
}

impl<S> WorkspaceBuilder<S> {
    pub fn new(workspace_id: uuid::Uuid) -> Self {
        Self {
            workspace_id: Some(workspace_id),
            config_manager: None,
            secrets_manager: None,
            intent_classifier: None,
        }
    }

    pub fn with_secrets_manager(mut self, secret_manager: SecretsManager) -> Self {
        self.secrets_manager = Some(secret_manager);
        self
    }

    pub async fn try_with_intent_classifier(mut self) -> Self {
        let config = IntentConfig::from_env();
        if !config.openai_api_key.is_empty() {
            match IntentClassifier::new(config).await {
                Ok(classifier) => {
                    self.intent_classifier = Some(Arc::new(classifier));
                }
                Err(e) => {
                    tracing::warn!("Failed to create intent classifier: {}", e);
                }
            }
        }
        self
    }

    pub async fn build(self) -> Result<WorkspaceManager<S>, OxyError> {
        let config_manager = self.config_manager.ok_or(OxyError::RuntimeError(
            "Config source is required".to_string(),
        ))?;

        let secret_manager = self
            .secrets_manager
            .unwrap_or(SecretsManager::from_environment().unwrap());

        let workspace_id = self.workspace_id.ok_or(OxyError::RuntimeError(
            "Workspace ID is required".to_string(),
        ))?;

        Ok(WorkspaceManager::new(
            workspace_id,
            config_manager,
            secret_manager,
            self.intent_classifier,
        ))
    }
}

/// `Origin` from the revision the caller resolved, or `Disk` when there is none.
/// Kept here rather than at the call site so `workspace_id` — which the builder
/// already holds — is never threaded through by hand.
fn origin_for(workspace_id: Option<uuid::Uuid>, revision_id: Option<uuid::Uuid>) -> Origin {
    match revision_id {
        Some(revision_id) => Origin::Compiled {
            workspace_id: workspace_id.unwrap_or_default(),
            revision_id,
        },
        None => Origin::Disk,
    }
}

impl WorkspaceBuilder<ReadOnly> {
    pub async fn without_working_copy<P: AsRef<Path>>(
        mut self,
        workspace_path: P,
        revision_id: Option<uuid::Uuid>,
        on_missing: OnMissing,
    ) -> Result<Self, OxyError> {
        let origin = origin_for(self.workspace_id, revision_id);
        self.config_manager = Some(
            ConfigBuilder::new()
                .with_workspace_path(workspace_path)?
                .build_without_working_copy(origin, on_missing)
                .await?,
        );
        Ok(self)
    }
}

impl WorkspaceBuilder<WorkingCopy> {
    pub async fn with_working_copy<P: AsRef<Path>>(
        mut self,
        workspace_path: P,
        revision_id: Option<uuid::Uuid>,
        on_missing: OnMissing,
    ) -> Result<Self, OxyError> {
        let origin = origin_for(self.workspace_id, revision_id);
        self.config_manager = Some(
            ConfigBuilder::new()
                .with_workspace_path(workspace_path)?
                .build_with_working_copy(origin, on_missing)
                .await?,
        );
        Ok(self)
    }
}

impl WorkspaceBuilder<WorkingCopy> {
    /// See [`ConfigBuilder::build_with_provided_config_and_working_copy`] — for a
    /// caller that already holds the parsed `Config`, not for request paths.
    pub fn with_working_copy_and_provided_config<P: AsRef<Path>>(
        mut self,
        workspace_path: P,
        config: crate::config::model::Config,
        revision_id: uuid::Uuid,
    ) -> Result<Self, OxyError> {
        let origin = origin_for(self.workspace_id, Some(revision_id));
        self.config_manager = Some(
            ConfigBuilder::new()
                .with_workspace_path(workspace_path)?
                .build_with_provided_config_and_working_copy(config, origin)?,
        );
        Ok(self)
    }
}
