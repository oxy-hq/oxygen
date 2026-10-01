use super::types::{APP_DATA_EXTENSION, APP_FILE_EXTENSION, AppResult, DATA_DIR_NAME};
use oxy::config::model::Task;
use oxy::config::{ConfigManager, DiskSlot, WorkingCopy};
use oxy::exec_types::{DataContainer, OutputContainer};
use oxy_shared::errors::OxyError;
use std::collections::HashMap;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use uuid::Uuid;
use xxhash_rust::xxh3::xxh3_64;

pub struct AppCache<S = WorkingCopy> {
    config_manager: ConfigManager<S>,
    /// Owning workspace — scopes the S3 mirror key so a stateless serve replica
    /// can read back a cache written by the ide.
    workspace_id: Uuid,
}

impl<S: DiskSlot> AppCache<S> {
    pub fn new(config_manager: ConfigManager<S>, workspace_id: Uuid) -> Self {
        Self {
            config_manager,
            workspace_id,
        }
    }

    /// Best-effort mirror of a just-written data cache to S3 so another fleet
    /// node (a stateless serve replica) can serve it when the ide is down.
    /// No-op when no blob bucket is configured (dev / single node).
    async fn mirror_cache(&self, data: &DataContainer, rel_path: &Path) {
        let Ok(yaml) = serde_yaml::to_string(data) else {
            return;
        };
        let key = crate::server::runtime_artifact::app_data_key(
            self.workspace_id,
            &rel_path.to_string_lossy(),
        );
        crate::server::runtime_artifact::mirror(&key, yaml.into_bytes(), "application/x-yaml")
            .await;
    }

    pub async fn clean_up_data(&self, app_path: &PathBuf, tasks: &[Task]) -> AppResult<()> {
        let (data_path, _) = self.get_data_paths(app_path, tasks)?;
        let state_dir = self.config_manager.runtime_state_dir();
        let data_path = state_dir.join(data_path);
        if data_path.exists() {
            std::fs::remove_dir_all(&data_path).map_err(|e| {
                OxyError::RuntimeError(format!("Failed to remove data directory: {e}"))
            })?;
        }
        Ok(())
    }

    pub async fn try_load_data(&self, app_path: &PathBuf, tasks: &[Task]) -> Option<DataContainer> {
        let (_data_path, data_file_path) = self.get_data_paths(app_path, tasks).ok()?;

        let state_dir = self.config_manager.runtime_state_dir();

        let full_cache_path = state_dir.join(&data_file_path);

        if !full_cache_path.exists() {
            // Cross-node read: a stateless serve replica didn't write this cache,
            // so fall back to the S3 mirror (None when no bucket is configured).
            let key = crate::server::runtime_artifact::app_data_key(
                self.workspace_id,
                &data_file_path.to_string_lossy(),
            );
            let bytes = crate::server::runtime_artifact::fetch(&key).await?;
            return serde_yaml::from_slice(&bytes).ok();
        }

        self.load_from_file(&full_cache_path)
    }

    /// Save a pre-built [`DataContainer`] directly, skipping the
    /// `OutputContainer::to_data` conversion that writes parquet from
    /// arrow batches.
    ///
    /// Used by the inline-automation app path where step results arrive
    /// as plain JSON (`{columns: [...], rows: [...]}` for tabular
    /// tasks) — there are no arrow batches to write, but we still want
    /// the `{file_path, json}` shape on the wire so the frontend can
    /// register the result in DuckDB-WASM by reading the inline JSON.
    /// The caller is responsible for shaping `data` correctly.
    pub async fn save_data_container(
        &self,
        app_path: &PathBuf,
        tasks: &[Task],
        data: DataContainer,
    ) -> AppResult<DataContainer> {
        let (_data_path, data_file_path) = self.get_data_paths(app_path, tasks)?;
        let state_dir = self.config_manager.runtime_state_dir();
        let full_cache_path = state_dir.join(&data_file_path);
        if let Some(parent) = full_cache_path.parent() {
            self.ensure_directory(&parent.to_path_buf())?;
        }
        self.save_to_file(&data, &full_cache_path)?;
        self.mirror_cache(&data, &data_file_path).await;
        Ok(data)
    }

    /// Params-scoped sibling of [`save_data_container`] — same skip-conversion
    /// shortcut but the cache file lands under a `params_<hash>` directory so
    /// the main result is preserved.
    pub async fn convert_to_data_container(
        &self,
        _app_path: &PathBuf,
        _tasks: &[Task],
        _params: &HashMap<String, serde_json::Value>,
        data: DataContainer,
    ) -> AppResult<DataContainer> {
        // Params-scoped runs deliberately don't persist to disk — same
        // semantics as `convert_to_data`, which only writes the parquet
        // shards (not the YAML cache). For pre-built DataContainers
        // there's nothing to write, so this is a passthrough.
        Ok(data)
    }

    pub async fn save_data(
        &self,
        app_path: &PathBuf,
        tasks: &[Task],
        output_container: OutputContainer,
    ) -> AppResult<DataContainer> {
        let (data_path, data_file_path) = self.get_data_paths(app_path, tasks)?;
        let state_dir = self.config_manager.runtime_state_dir();

        let full_data_path = state_dir.join(&data_path);
        self.ensure_directory(&full_data_path)?;

        let data = output_container.to_data(&data_path, &state_dir)?;

        let full_cache_path = state_dir.join(&data_file_path);

        self.save_to_file(&data, &full_cache_path)?;
        self.mirror_cache(&data, &data_file_path).await;

        Ok(data)
    }

    /// Converts an OutputContainer to DataContainer without touching the main cache.
    /// Writes parquet files to a params-specific subdirectory so the main cache is preserved.
    pub async fn convert_to_data(
        &self,
        app_path: &PathBuf,
        tasks: &[Task],
        params: &HashMap<String, serde_json::Value>,
        output_container: OutputContainer,
    ) -> AppResult<DataContainer> {
        let (data_path, _) = self.get_data_paths(app_path, tasks)?;
        let params_hash = self.generate_params_hash(params)?;
        let params_data_path = data_path.join(format!("params_{params_hash}"));

        let state_dir = self.config_manager.runtime_state_dir();
        let full_params_data_path = state_dir.join(&params_data_path);
        self.ensure_directory(&full_params_data_path)?;

        let data = output_container.to_data(&params_data_path, &state_dir)?;
        Ok(data)
    }

    fn generate_params_hash(
        &self,
        params: &HashMap<String, serde_json::Value>,
    ) -> AppResult<String> {
        // Use a sorted serialization for deterministic hashing
        let mut sorted: Vec<_> = params.iter().collect();
        sorted.sort_by_key(|(k, _)| k.as_str());
        let serialized = serde_json::to_string(&sorted)
            .map_err(|e| OxyError::RuntimeError(format!("Failed to serialize params: {e}")))?;
        Ok(format!("{:x}", xxh3_64(serialized.as_bytes())))
    }

    fn get_data_paths(&self, app_path: &PathBuf, tasks: &[Task]) -> AppResult<(PathBuf, PathBuf)> {
        tracing::debug!("Getting app data path: {app_path:?}");
        let full_path = app_path;

        let file_name = full_path
            .file_name()
            .ok_or_else(|| OxyError::ConfigurationError("Invalid file path".to_string()))?
            .to_string_lossy()
            .to_string();

        if !file_name.ends_with(APP_FILE_EXTENSION) {
            return Err(OxyError::ConfigurationError(format!(
                "File must have {APP_FILE_EXTENSION} extension"
            )));
        }

        let tasks_hash = self.generate_task_hash(tasks)?;
        // Partitioned under a staging pin (a workspace preview): the same tasks
        // can answer differently against a branch's semantic model, and this
        // file is shared by every reader of the app.
        let partition = crate::server::api::custom_apps_staging_pin::cache_partition();
        let tasks_hash = if partition.is_empty() {
            tasks_hash
        } else {
            format!(
                "{:x}",
                xxh3_64(format!("{tasks_hash}|{partition}").as_bytes())
            )
        };
        let data_file_name = format!(
            "{}.{}",
            tasks_hash,
            file_name.replace(APP_FILE_EXTENSION, APP_DATA_EXTENSION)
        );

        let directory_name = file_name.replace(APP_FILE_EXTENSION, "");
        let data_path: PathBuf = full_path
            .parent()
            .ok_or_else(|| OxyError::RuntimeError("Invalid file path structure".to_string()))?
            .join(DATA_DIR_NAME)
            .join(directory_name);
        let data_file_path = data_path.join(data_file_name);

        Ok((data_path, data_file_path))
    }

    fn generate_task_hash(&self, tasks: &[Task]) -> AppResult<String> {
        let tasks_serialized = serde_json::to_string(tasks)
            .map_err(|e| OxyError::RuntimeError(format!("Failed to serialize tasks: {e}")))?;
        let tasks_hash = xxh3_64(tasks_serialized.as_bytes());
        Ok(format!("{tasks_hash:x}"))
    }

    fn ensure_directory(&self, data_path: &PathBuf) -> AppResult<()> {
        if !data_path.exists() {
            std::fs::create_dir_all(data_path).map_err(|e| {
                OxyError::RuntimeError(format!("Failed to create data directory: {e}"))
            })?;
        }
        Ok(())
    }

    fn save_to_file(&self, data: &DataContainer, file_path: &PathBuf) -> AppResult<()> {
        let data_file = std::fs::File::create(file_path)
            .map_err(|e| OxyError::RuntimeError(format!("Failed to create data file: {e}")))?;
        let writer = BufWriter::new(data_file);
        serde_yaml::to_writer(writer, data)
            .map_err(|e| OxyError::RuntimeError(format!("Failed to write data to file: {e}")))?;
        Ok(())
    }

    fn load_from_file(&self, file_path: &PathBuf) -> Option<DataContainer> {
        let file = std::fs::File::open(file_path).ok()?;
        let reader = BufReader::new(file);
        match serde_yaml::from_reader(reader) {
            Ok(data) => Some(data),
            Err(e) => {
                tracing::warn!("Failed to parse data file: {}", e);
                None
            }
        }
    }
}
