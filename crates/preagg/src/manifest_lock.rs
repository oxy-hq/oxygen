//! The per-workspace manifest write lock, out of `oxy-app`'s
//! `server::preagg_workspace` (2026-11-21), which re-exported it until it went
//! down into `oxy-workspace-runtime` (2026-11-22); the
//! executor shell hands each cycle the lock for its workspace, and
//! [`crate::rebuild`] publishes the manifest under it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use tokio::sync::Mutex as TokioMutex;
use uuid::Uuid;

/// Per-workspace manifest write lock, keyed by workspace id.
///
/// `manifest.json` is one file per workspace's local cache directory; every
/// rebuild rewrites it, so any two cycles touching the SAME workspace — a
/// scheduled fire racing an on-demand "Rebuild" click — must serialize. Two
/// DIFFERENT workspaces' cycles must not: they write to different directories
/// and a single global lock would only add queueing with no correctness
/// benefit, on a fleet where many workspaces' cycles can legitimately run at
/// once.
pub fn manifest_write_lock_for(workspace_id: Uuid) -> Arc<TokioMutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<Uuid, Arc<TokioMutex<()>>>>> = OnceLock::new();
    let mut locks = LOCKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("preagg manifest lock registry poisoned");
    Arc::clone(
        locks
            .entry(workspace_id)
            .or_insert_with(|| Arc::new(TokioMutex::new(()))),
    )
}
