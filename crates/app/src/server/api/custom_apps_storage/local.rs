//! Filesystem backend for the custom-app asset store — local development
//! without S3/MinIO.
//!
//! Mirrors the build store's fallback: keys become paths under `OXY_STATE_DIR`, so
//! the same key works on both backends. Presigning has no filesystem analogue (it
//! mints a URL the *browser* uses), so those two calls are S3-only; everything
//! else behaves the same here.
//!
//! Single-node only, exactly like the build store's FS mode: on a multi-replica
//! deployment a later request lands on a replica whose local disk has no such
//! object. Configure a bucket for anything beyond one process.

use std::path::{Path, PathBuf};

use super::{ListPage, StorageError, StoredObject};

/// Local-mode root, shared with the build store so `OXY_STATE_DIR` governs both.
fn state_root() -> PathBuf {
    if let Ok(dir) = std::env::var("OXY_STATE_DIR")
        && !dir.trim().is_empty()
    {
        return PathBuf::from(dir);
    }
    dirs::data_local_dir()
        .map(|d| d.join("oxy"))
        .unwrap_or_else(|| PathBuf::from(".oxy-state"))
}

/// On-disk path for an already-validated key.
fn path_for(key: &str) -> PathBuf {
    state_root().join(key)
}

pub(super) async fn put(
    key: &str,
    body: Vec<u8>,
    allow_overwrite: bool,
) -> Result<(), StorageError> {
    let dest = path_for(key);
    if !allow_overwrite && tokio::fs::try_exists(&dest).await.unwrap_or(false) {
        return Err(StorageError::AlreadyExists(format!(
            "'{key}' already exists; pass allowOverwrite to replace it or \
             addRandomSuffix to store alongside it"
        )));
    }
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| StorageError::Io(format!("mkdir {}: {e}", parent.display())))?;
    }
    tokio::fs::write(&dest, body)
        .await
        .map_err(|e| StorageError::Io(format!("write {}: {e}", dest.display())))
}

pub(super) async fn get(key: &str) -> Result<Option<(Vec<u8>, Option<String>)>, StorageError> {
    let dest = path_for(key);
    match tokio::fs::read(&dest).await {
        Ok(bytes) => Ok(Some((
            bytes,
            Some(super::guess_content_type(key).to_string()),
        ))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(StorageError::Io(format!("read {}: {e}", dest.display()))),
    }
}

pub(super) async fn head(key: &str) -> Result<Option<StoredObject>, StorageError> {
    let dest = path_for(key);
    match tokio::fs::metadata(&dest).await {
        Ok(meta) => Ok(Some(StoredObject {
            key: key.to_string(),
            size: meta.len() as i64,
            content_type: Some(super::guess_content_type(key).to_string()),
            last_modified: meta
                .modified()
                .ok()
                .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339()),
        })),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(StorageError::Io(format!("stat {}: {e}", dest.display()))),
    }
}

/// Paginated listing over the prefix directory. Keys are sorted so the cursor
/// (an offset) is stable across calls — S3 returns lexicographic order, and
/// matching that keeps app code behaving the same on both backends.
pub(super) fn list(
    prefix: &str,
    limit: usize,
    cursor: Option<String>,
) -> Result<ListPage, StorageError> {
    let root = state_root();
    let mut keys = Vec::new();
    collect(&root, &root.join(prefix), &mut keys)?;
    keys.sort_by(|a, b| a.key.cmp(&b.key));

    let offset: usize = cursor
        .as_deref()
        .filter(|c| !c.is_empty())
        .map(|c| c.parse().unwrap_or(0))
        .unwrap_or(0);
    let end = offset.saturating_add(limit).min(keys.len());
    let page: Vec<StoredObject> = keys.get(offset..end).unwrap_or(&[]).to_vec();
    let has_more = end < keys.len();
    Ok(ListPage {
        objects: page,
        cursor: has_more.then(|| end.to_string()),
        has_more,
    })
}

/// Walk `dir` collecting every file as a key relative to `root`.
fn collect(root: &Path, dir: &Path, out: &mut Vec<StoredObject>) -> Result<(), StorageError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        // A prefix with nothing under it lists empty, not an error.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(StorageError::Io(format!("read_dir {}: {e}", dir.display()))),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, out)?;
        } else if let Ok(meta) = entry.metadata()
            && let Ok(rel) = path.strip_prefix(root)
        {
            let key = rel.to_string_lossy().replace('\\', "/");
            out.push(StoredObject {
                size: meta.len() as i64,
                content_type: Some(super::guess_content_type(&key).to_string()),
                last_modified: meta
                    .modified()
                    .ok()
                    .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339()),
                key,
            });
        }
    }
    Ok(())
}

pub(super) async fn delete(keys: &[String]) -> Result<usize, StorageError> {
    let mut deleted = 0;
    for key in keys {
        match tokio::fs::remove_file(path_for(key)).await {
            // Count = keys ACCEPTED for deletion, so this agrees with the S3
            // backend: deletion is idempotent, so an absent key counts too (S3
            // reports it as deleted, and can't cheaply say it was absent).
            Ok(()) => deleted += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => deleted += 1,
            Err(e) => return Err(StorageError::Io(format!("remove {key}: {e}"))),
        }
    }
    Ok(deleted)
}

pub(super) async fn copy(from: &str, to: &str, allow_overwrite: bool) -> Result<(), StorageError> {
    let dest = path_for(to);
    if !allow_overwrite && tokio::fs::try_exists(&dest).await.unwrap_or(false) {
        return Err(StorageError::AlreadyExists(format!(
            "'{to}' already exists; pass allowOverwrite to replace it"
        )));
    }
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| StorageError::Io(format!("mkdir {}: {e}", parent.display())))?;
    }
    tokio::fs::copy(path_for(from), &dest)
        .await
        .map_err(|e| match e.kind() {
            // The destination's directory exists by now, so a missing path is
            // the source — the same shape the object store raises.
            std::io::ErrorKind::NotFound => {
                StorageError::NotFound(format!("copy source '{from}' does not exist"))
            }
            _ => StorageError::Io(format!("copy {from} -> {to}: {e}")),
        })?;
    Ok(())
}

pub(super) async fn delete_prefix(prefix: &str) -> Result<(), StorageError> {
    let dir = state_root().join(prefix);
    match tokio::fs::remove_dir_all(&dir).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(StorageError::Io(format!("rmdir {}: {e}", dir.display()))),
    }
}
