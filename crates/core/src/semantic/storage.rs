use std::{collections::HashMap, path::PathBuf};

use tokio::fs::{read_dir, read_to_string};

use crate::config::{ConfigManager, constants::SEMANTIC_MODEL_PATH};
use oxy_shared::errors::OxyError;

use super::types::{DatasetInfo, SemanticKey};
use crate::config::WorkingCopy;

#[enum_dispatch::enum_dispatch]
trait Storage {
    async fn load_datasets(&self, database_ref: &str) -> Result<Vec<DatasetInfo>, OxyError>;
    async fn load_ddl(&self, key: &SemanticKey) -> Result<String, OxyError>;
    async fn load_raw_models(&self, key: &SemanticKey)
    -> Result<HashMap<String, String>, OxyError>; // Loading plaintext to allow easy iterate through the model
}

pub struct SemanticFileStorage {
    base_path: String,
    semantic_path: String,
}

impl SemanticFileStorage {
    pub async fn from_config(config: &ConfigManager<WorkingCopy>) -> Result<Self, OxyError> {
        let base_path = config.database_semantic_path();

        Ok(SemanticFileStorage {
            base_path: base_path.to_string_lossy().to_string(),
            semantic_path: SEMANTIC_MODEL_PATH.to_string(),
        })
    }

    fn get_database_dir(&self, database_ref: &str) -> PathBuf {
        PathBuf::from(&self.base_path).join(database_ref)
    }

    fn get_dataset_ddl_path(&self, key: &SemanticKey) -> PathBuf {
        self.get_database_dir(&key.database)
            .join(format!("{}.sql", key.dataset))
    }

    fn get_dataset_semantic_dir(&self, key: &SemanticKey) -> PathBuf {
        self.get_database_dir(&key.database)
            .join(&key.dataset)
            .join(&self.semantic_path)
    }
}

impl Storage for SemanticFileStorage {
    async fn load_datasets(&self, database_ref: &str) -> Result<Vec<DatasetInfo>, OxyError> {
        let db_dir = self.get_database_dir(database_ref);
        if !db_dir.exists() {
            // Two answers, and the caller maps `IOError` to "not synced yet".
            // That is right on a node that owns the working copy: the sync
            // directory is simply not there because nobody has synced. On a node
            // that owns no working copy it is a guess presented as a fact, and
            // gets an error the caller does not swallow.
            //
            // The predicate is the process, not the directory. Testing
            // `base_path.is_dir()` reads an ide that was never synced as "cannot
            // know", which trades one wrong answer for another.
            if !crate::workspace_fs_probe::process_owns_workspace_files() {
                return Err(OxyError::ConfigurationError(format!(
                    "semantic sync directory {} is not on this node, so whether \
                     {database_ref} is synced is unknown",
                    self.base_path
                )));
            }
            return Err(OxyError::IOError(format!(
                "Failed to load file: {}",
                db_dir.display()
            )));
        }
        let mut read_dir = read_dir(db_dir)
            .await
            .map_err(|err| OxyError::IOError(format!("Failed to list database path: {err}")))?;

        let mut results = vec![];
        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(|err| OxyError::IOError(format!("Failed to read database directory: {err}")))?
        {
            let file_type = entry.file_type().await.map_err(|err| {
                OxyError::IOError(format!("Failed to read database file type: {err}"))
            })?;
            if file_type.is_dir() {
                let dataset = entry.file_name().to_string_lossy().to_string();
                let key = SemanticKey::new(database_ref.to_string(), dataset.clone());
                let ddl = self.load_ddl(&key).await.ok();
                let semantic_info = self.load_raw_models(&key).await?;
                results.push(DatasetInfo {
                    dataset,
                    ddl,
                    semantic_info,
                });
            }
        }
        Ok(results)
    }

    async fn load_ddl(&self, key: &SemanticKey) -> Result<String, OxyError> {
        // Implement file loading logic here
        let ddl_path = self.get_dataset_ddl_path(key);
        if !ddl_path.exists() {
            return Err(OxyError::IOError(format!(
                "Failed to load file: {}",
                ddl_path.display()
            )));
        }
        let ddl = read_to_string(ddl_path)
            .await
            .map_err(|err| OxyError::IOError(format!("Failed to load ddl file: {err}")))?;
        Ok(ddl)
    }

    async fn load_raw_models(
        &self,
        key: &SemanticKey,
    ) -> Result<HashMap<String, String>, OxyError> {
        // Implement file loading logic here
        let semantic_path = self.get_dataset_semantic_dir(key);
        if !semantic_path.exists() {
            return Err(OxyError::IOError(format!(
                "Failed to load file: {}",
                semantic_path.display()
            )));
        }
        let mut read_dir = read_dir(semantic_path)
            .await
            .map_err(|err| OxyError::IOError(format!("Failed to list semantic path: {err}")))?;
        let mut results = HashMap::new();
        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(|err| OxyError::IOError(format!("Failed to read semantic directory: {err}")))?
        {
            let file_type = entry.file_type().await.map_err(|err| {
                OxyError::IOError(format!("Failed to read semantic file type: {err}"))
            })?;
            if file_type.is_file()
                && entry
                    .file_name()
                    .to_string_lossy()
                    .to_string()
                    .ends_with(".schema.yml")
            {
                let semantic_file_path = entry.path();
                let key = semantic_file_path
                    .file_stem()
                    .unwrap()
                    .to_string_lossy()
                    .split(".")
                    .next()
                    .unwrap_or_default()
                    .to_string();
                let semantic_model = read_to_string(semantic_file_path).await.map_err(|err| {
                    OxyError::IOError(format!("Failed to load semantic file: {err}"))
                })?;
                results.insert(key, semantic_model);
            }
        }

        Ok(results)
    }
}

#[enum_dispatch::enum_dispatch(Storage)]
pub enum StorageImpl {
    File(SemanticFileStorage),
}

pub struct SemanticStorage {
    storage: StorageImpl,
}

impl SemanticStorage {
    pub async fn from_config(config: &ConfigManager<WorkingCopy>) -> Result<Self, OxyError> {
        let storage = SemanticFileStorage::from_config(config).await?;
        Ok(SemanticStorage {
            storage: StorageImpl::File(storage),
        })
    }

    pub async fn load_datasets(&self, database_ref: &str) -> Result<Vec<DatasetInfo>, OxyError> {
        self.storage.load_datasets(database_ref).await
    }
}
