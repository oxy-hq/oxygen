//! Save an archive response to disk under a size limit.

use std::path::Path;

use oxy::github::{TarballError, transport_error};
use reqwest::Response;
use tokio::io::AsyncWriteExt;

use super::CompileGitError;

/// The largest archive that will be downloaded: 512 MiB, compressed.
///
/// The archive and the tree it unpacks to are on disk together until the
/// unpack finishes, so with [`super::MAX_TREE_BYTES`] this bounds what one
/// compile can take from a pod's ephemeral disk at 1.5 GiB. It is not smaller
/// because committed Parquet barely compresses: an archive of data files is
/// about the size of the files.
pub const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;

/// Stream `response`'s body into `into`, failing as soon as it passes `limit`.
/// The limit is counted on the bytes that arrive; `Content-Length` only lets
/// an honest server be refused before the first byte.
pub(super) async fn save_body(
    mut response: Response,
    into: &Path,
    limit: u64,
) -> Result<u64, CompileGitError> {
    let too_large = || CompileGitError::ArchiveTooLarge { limit };
    if response.content_length().is_some_and(|n| n > limit) {
        return Err(too_large());
    }
    let mut file = tokio::fs::File::create(into)
        .await
        .map_err(CompileGitError::io)?;
    let mut written: u64 = 0;
    while let Some(chunk) = response.chunk().await.map_err(|e| {
        let cause = transport_error(e);
        TarballError::Unavailable(format!("the archive download broke off: {cause}"))
    })? {
        written += chunk.len() as u64;
        if written > limit {
            return Err(too_large());
        }
        file.write_all(&chunk).await.map_err(CompileGitError::io)?;
    }
    file.flush().await.map_err(CompileGitError::io)?;
    Ok(written)
}
