//! Multi-instance read-through for runtime-generated FS artifacts.
//!
//! Some artifacts are written to a node's LOCAL filesystem during a
//! request/run flow and read back by a LATER HTTP request — query-result
//! parquet files, chart PNGs. On a stateless serve fleet the read
//! round-robins across replicas, so the node that fetches the artifact is
//! usually NOT the one that wrote it → 404 / blank render.
//!
//! Rather than rewrite every producer to be S3-native, this module adds a
//! cheap, additive safety net:
//!
//!   - On WRITE: after the local file is written, best-effort mirror the
//!     bytes to S3 ([`mirror`]). Failures are logged, never fatal.
//!   - On READ: serve the local file when present; on a local miss, fall
//!     back to S3 ([`fetch`]). The local path stays the fast path; S3 is
//!     only consulted for the cross-node case.
//!
//! Storage reuses the **compile-boundary blob bucket**
//! (`OXY_COMPILE_BLOB_S3_BUCKET`, already provisioned and readable by every
//! fleet pod) via [`oxy_compile::blob_store`] — no new bucket, no new env
//! var. When the bucket is unset (local dev / single node) every S3 op is a
//! no-op and behavior is byte-identical to before: the local file is the
//! only copy and is always served from the node that wrote it.
//!
//! Keys are workspace-scoped so the shared bucket stays multi-tenant-safe.

use uuid::Uuid;

/// S3 key for a query-result parquet file.
pub fn result_key(workspace_id: Uuid, file_name: &str) -> String {
    format!("runtime/results/{workspace_id}/{file_name}")
}

/// S3 key for a chart image.
pub fn chart_key(workspace_id: Uuid, file_name: &str) -> String {
    format!("runtime/charts/{workspace_id}/{file_name}")
}

/// Best-effort mirror of a just-written artifact to S3. No-op when no bucket
/// is configured (dev / single-node). Never fails the caller — a mirror
/// failure only costs the cross-node read fallback, which logs and 404s the
/// same as today.
pub async fn mirror(key: &str, bytes: Vec<u8>, content_type: &str) {
    match oxy_compile::blob_store::put_object_at_key(key, bytes, content_type).await {
        Ok(Some(_)) => tracing::debug!(key, "runtime artifact mirrored to S3"),
        // No bucket configured — local FS is the only store (dev/single node).
        Ok(None) => {}
        Err(e) => tracing::warn!(
            error = %e,
            key,
            "runtime artifact S3 mirror failed (best-effort; cross-node reads of this \
             artifact will 404 until re-generated)"
        ),
    }
}

/// Read-through fetch from S3, used only when the local file is missing (the
/// cross-node case). Returns `None` when no bucket is configured or the
/// object isn't there / a transport error occurs — the caller then 404s,
/// exactly as it would have without this module.
pub async fn fetch(key: &str) -> Option<Vec<u8>> {
    match oxy_compile::blob_store::get_blob(key).await {
        Ok(Some(bytes)) => Some(bytes),
        Ok(None) => None,
        Err(e) => {
            // A genuine miss (NoSuchKey) surfaces here as a transport error
            // too; either way there's nothing to serve. Debug, not warn —
            // this is the expected path when an artifact was never mirrored.
            tracing::debug!(error = %e, key, "runtime artifact S3 read-through miss");
            None
        }
    }
}

/// Remove the mirrored copy, so a delete on one node is a delete everywhere.
///
/// Best-effort and idempotent: `false` means no bucket is configured (a single
/// node has nothing to mirror) or the removal failed, and neither is worth
/// failing the caller's delete over — the local file is already gone. It is
/// worth a WARN, though: a mirror that outlives its local file is served to
/// every replica by [`fetch`], which is the shape where a user is told an
/// artifact was deleted and it keeps loading.
pub async fn remove(key: &str) -> bool {
    match oxy_compile::blob_store::delete_object(key).await {
        Ok(deleted) => deleted,
        Err(e) => {
            tracing::warn!(
                error = %e, key,
                "runtime artifact S3 removal failed; the mirrored copy will still be served"
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_workspace_scoped() {
        let ws = Uuid::nil();
        assert_eq!(
            result_key(ws, "abc.parquet"),
            "runtime/results/00000000-0000-0000-0000-000000000000/abc.parquet"
        );
        assert_eq!(
            chart_key(ws, "sales-0-xyz.png"),
            "runtime/charts/00000000-0000-0000-0000-000000000000/sales-0-xyz.png"
        );
    }
}
