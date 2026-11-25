//! Where served react runs store their query results
//! (`agentic_runtime::react::ResultArchive`): one JSON object per result at
//! `runtime/react-results/<workspace>/<run>/<rN>.json`.
//!
//! - **With a bucket** (`OXY_COMPILE_BLOB_S3_BUCKET`, every fleet pod reads it)
//!   the object is the only copy, so any replica serves it. The leading prefix
//!   is the results' own, so a bucket lifecycle rule on
//!   `runtime/react-results/` can expire them after 30 days without touching
//!   the SQL IDE's files under `runtime/results/`. Reads enforce the 30 days
//!   either way (`agentic_runtime::react::page`).
//! - **Without one** (local dev, one process) it is a file under the state
//!   directory.
//!
//! Unlike `runtime_artifact::mirror`, a write reports failure: the run logs it
//! and the table then answers "not found" for rows past its first page.

use std::path::PathBuf;

use agentic_runtime::react::{ArchiveError, ArchiveKey, ArchivedResult, ResultArchive};
use async_trait::async_trait;
use oxy_compile::blob_store;

const PREFIX: &str = "runtime/react-results";

/// The bucket when one is configured, else files under `local_root`.
pub struct BlobResultArchive {
    local_root: PathBuf,
}

impl BlobResultArchive {
    pub fn new(local_root: PathBuf) -> Self {
        Self { local_root }
    }

    /// Files under `<state dir>/react-results` when there is no bucket.
    pub fn from_env() -> Self {
        Self::new(oxy_shared::state_dir::state_dir_path(None).join("react-results"))
    }

    fn object_key(key: &ArchiveKey) -> String {
        format!("{PREFIX}/{}.json", key.path())
    }

    fn local_path(&self, key: &ArchiveKey) -> PathBuf {
        self.local_root.join(format!("{}.json", key.path()))
    }
}

fn storage(e: impl std::fmt::Display) -> ArchiveError {
    ArchiveError::Storage(e.to_string())
}

#[async_trait]
impl ResultArchive for BlobResultArchive {
    async fn put(&self, key: &ArchiveKey, result: &ArchivedResult) -> Result<(), ArchiveError> {
        let body = serde_json::to_vec(result).map_err(storage)?;
        if blob_store::bucket().is_some() {
            let object = Self::object_key(key);
            return blob_store::put_object_at_key(&object, body, "application/json")
                .await
                .map(|_| ())
                .map_err(storage);
        }
        let path = self.local_path(key);
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir).await.map_err(storage)?;
        }
        tokio::fs::write(&path, body).await.map_err(storage)
    }

    async fn get(&self, key: &ArchiveKey) -> Result<Option<ArchivedResult>, ArchiveError> {
        let body = if blob_store::bucket().is_some() {
            match blob_store::object_exists(&Self::object_key(key)).await {
                Ok(false) => return Ok(None),
                Ok(true) => {}
                Err(e) => return Err(storage(e)),
            }
            blob_store::get_blob(&Self::object_key(key))
                .await
                .map_err(storage)?
        } else {
            match tokio::fs::read(self.local_path(key)).await {
                Ok(bytes) => Some(bytes),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(storage(e)),
            }
        };
        body.map(|b| serde_json::from_slice(&b).map_err(storage))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_runtime::react::ArchiveKey;
    use serde_json::json;

    /// Without a bucket a result is a file: it reads back as written, and a
    /// key nothing was stored at reads as `None`.
    #[tokio::test]
    async fn a_stored_result_reads_back_from_local_files() {
        if blob_store::bucket().is_some() {
            return; // this checks the local path only
        }
        let dir = tempfile::tempdir().unwrap();
        let archive = BlobResultArchive::new(dir.path().to_path_buf());
        let ws = uuid::Uuid::new_v4();
        let run = uuid::Uuid::new_v4().to_string();
        let key = ArchiveKey::new(ws, &run, "r2").unwrap();
        let result = ArchivedResult {
            sql: "select 1".into(),
            columns: vec!["delta".into()],
            rows: std::sync::Arc::new(vec![vec![json!(-12.5)], vec![json!(null)]]),
            truncated: true,
            created_at: chrono::Utc::now(),
        };
        archive.put(&key, &result).await.unwrap();
        assert_eq!(archive.get(&key).await.unwrap(), Some(result));
        let other = ArchiveKey::new(ws, &run, "r3").unwrap();
        assert_eq!(archive.get(&other).await.unwrap(), None);
        assert!(dir.path().join(format!("{ws}/{run}/r2.json")).exists());
    }
}
