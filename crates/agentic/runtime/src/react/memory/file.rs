//! A note log in a JSON-lines file, one [`NoteFact`] per line: the eval
//! harness's store, kept between `oxy test` runs.

use std::path::PathBuf;

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;

use super::{MemoryError, MemoryStore, NoteFact};

/// Parallel runs each hold their own store on one file. Lines do not
/// interleave because the file is opened `O_APPEND` and each fact is one
/// short line written with one `write_all`.
pub struct FileMemoryStore {
    path: PathBuf,
}

impl FileMemoryStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

#[async_trait]
impl MemoryStore for FileMemoryStore {
    /// A missing file is an empty log.
    async fn facts(&self) -> Result<Vec<NoteFact>, MemoryError> {
        let text = match tokio::fs::read_to_string(&self.path).await {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let lines = text.lines().filter(|l| !l.trim().is_empty());
        Ok(lines.map(serde_json::from_str).collect::<Result<_, _>>()?)
    }

    async fn append(&self, fact: NoteFact) -> Result<(), MemoryError> {
        let mut line = serde_json::to_string(&fact)?;
        line.push('\n');
        let mut f = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await?;
        f.write_all(line.as_bytes()).await?;
        Ok(f.flush().await?)
    }
}
