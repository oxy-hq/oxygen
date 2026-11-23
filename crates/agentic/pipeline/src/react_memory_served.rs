//! Memory for a served react run: the requesting user's notes for the
//! workspace, in Oxy's Postgres (`agent_memory_notes`).
//!
//! Best effort: a run answers without notes rather than fail. No memory when
//! the org's `react_memory` flag is off or no user requested the run (a run
//! with no requester never shares a bucket). Read-only, with no memory tools
//! and no reflection turn, for a custom app's staging or sandbox ask, which
//! writes nothing for real; a workspace preview reads the flag as off.

use std::sync::Arc;

use agentic_runtime::react::{DbMemoryStore, MemoryStore, NoteOwner, RunMemory, latest};
use sea_orm::DatabaseConnection;

use crate::platform::PlatformContext;

pub(crate) async fn served_memory(
    platform: &dyn PlatformContext,
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
) -> RunMemory {
    if !platform.react_memory_enabled().await {
        return RunMemory::default();
    }
    let Some(requester) = platform.requester() else {
        return RunMemory::default();
    };
    let owner = NoteOwner {
        workspace_id,
        user_id: requester.user_id,
    };
    let store = DbMemoryStore::new(db.clone(), owner, None);
    let notes = match store.facts().await {
        Ok(facts) => latest(facts),
        Err(e) => {
            tracing::warn!(%workspace_id, error = %e, "react memory: notes unreadable; the run has none");
            return RunMemory::default();
        }
    };
    let writes = (!writes_held(platform)).then(|| Arc::new(store) as Arc<dyn MemoryStore>);
    // A chat reads the streamed answer; the reflection turn stays off it.
    let quiet = true;
    RunMemory {
        notes,
        writes,
        quiet,
    }
}

/// Whether `platform` holds writes, so a run on it saves no notes: a custom
/// app's staging or sandbox ask (`staging_app_id`), or a workspace preview.
pub(crate) fn writes_held(platform: &dyn PlatformContext) -> bool {
    platform.staging_app_id().is_some() || platform.is_workspace_preview()
}

#[cfg(test)]
#[path = "react_memory_served_tests.rs"]
mod tests;
