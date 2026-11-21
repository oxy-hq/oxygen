use std::collections::HashMap;

use crate::config::ConfigManager;
use oxy_shared::errors::OxyError;

use super::{
    SemanticVariablesContexts, contexts::SemanticDimensionsContexts, storage::SemanticStorage,
    types::DatabaseInfo,
};
use crate::config::WorkingCopy;

/// Reads what a sync left in the working copy's `.databases/`. Rendering a
/// sync is [`super::render`]'s, and writing one is the caller's (`oxyc sync`).
pub struct SemanticManager {
    config: ConfigManager<WorkingCopy>,
    storage: SemanticStorage,
}

impl SemanticManager {
    pub async fn from_config(config: ConfigManager<WorkingCopy>) -> Result<Self, OxyError> {
        let storage = SemanticStorage::from_config(&config).await?;
        Ok(SemanticManager { config, storage })
    }

    /// Try to load database info from cache only, without triggering sync.
    /// Returns None if the database hasn't been synced yet.
    pub async fn try_load_cached_database_info(
        &self,
        database_ref: &str,
    ) -> Result<Option<DatabaseInfo>, OxyError> {
        let database = self.config.resolve_database(database_ref)?;
        match self.storage.load_datasets(database_ref).await {
            Ok(datasets) => Ok(Some(DatabaseInfo {
                name: database_ref.to_string(),
                dialect: database.dialect(),
                datasets: datasets
                    .into_iter()
                    .map(|d| (d.dataset.clone(), d))
                    .collect(),
            })),
            Err(OxyError::IOError(_)) => {
                // Not synced yet, return None
                Ok(None)
            }
            Err(err) => Err(err),
        }
    }

    /// Build an empty `SemanticVariablesContexts` for jinja template rendering.
    ///
    /// Previously this enumerated `dimensions[].targets` from `semantics.yml` and
    /// hydrated per-table dimension schemas. With the global semantics registry
    /// removed, callers get an empty context — `{{ models.<table>.* }}` template
    /// references resolve to undefined.
    pub async fn get_semantic_variables_contexts(
        &self,
    ) -> Result<SemanticVariablesContexts, OxyError> {
        SemanticVariablesContexts::new(HashMap::new())
    }

    /// Build an empty `SemanticDimensionsContexts` for jinja template rendering.
    ///
    /// Previously this exposed dimensions declared in `semantics.yml`. With the
    /// global semantics registry removed, callers get an empty context.
    pub async fn get_semantic_dimensions_contexts(
        &self,
    ) -> Result<SemanticDimensionsContexts, OxyError> {
        Ok(SemanticDimensionsContexts {
            dimensions: HashMap::new(),
        })
    }
}
