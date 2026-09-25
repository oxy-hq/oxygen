//! Per-workspace default-branch resolver.
//!
//! The compile boundary serves Postgres-backed reads only when the
//! request is for the workspace's *default* branch (typically `main`,
//! sometimes `master` or a custom name). Non-default branches read
//! from FS — the IDE working copy is the freshest source by definition.
//!
//! The default branch is discovered from the local checkout via
//! `GitClient::get_default_branch` (which reads `refs/remotes/origin/HEAD`
//! under the hood). That's a sub-millisecond operation on a healthy clone
//! but still a syscall; we cache the result per-workspace with a small
//! TTL so the hybrid reader can call it on every request without cost.
//!
//! When the workspace is missing, its path is unset, or the lookup
//! errors, `resolve_default_branch` returns `None`. The hybrid reader
//! treats that as "I can't classify the branch — fall through to FS"
//! (`is_default_branch` returns `false` on `None`). Reading from FS
//! is the safer answer here: the branch-aware contract exists so
//! feature-branch users see their working-copy edits, and silently
//! serving the promoted main revision to a request that can't be
//! classified would violate that contract. The cache + GitClient
//! both fail to FS, never to stale Postgres.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use oxy_git::GitClient;
use sea_orm::{DatabaseConnection, EntityTrait};
use uuid::Uuid;

/// How long a per-workspace default-branch entry is reused before we
/// re-discover it from the git client. Short enough to pick up a
/// `git symbolic-ref` change inside a few seconds (rare op), long
/// enough that the typical request burst is one syscall amortised.
const CACHE_TTL: Duration = Duration::from_secs(60);

#[derive(Clone)]
struct CachedBranch {
    name: String,
    fetched_at: Instant,
}

static CACHE: OnceLock<RwLock<HashMap<Uuid, CachedBranch>>> = OnceLock::new();

fn cache() -> &'static RwLock<HashMap<Uuid, CachedBranch>> {
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Returns the workspace's default branch name (e.g. `"main"`). `None`
/// when the workspace doesn't exist, has no path, or the git lookup
/// fails — caller should treat that as "use the safer fallback path."
pub async fn resolve_default_branch(db: &DatabaseConnection, workspace_id: Uuid) -> Option<String> {
    if let Some(hit) = read_cache(workspace_id) {
        return Some(hit);
    }

    let workspace_row = entity::workspaces::Entity::find_by_id(workspace_id)
        .one(db)
        .await
        .ok()
        .flatten()?;
    let path = workspace_row.path.as_deref()?;
    let workspace_path = std::path::Path::new(path);

    let client = oxy::github::default_git_client();
    let branch = client.get_default_branch(workspace_path).await;
    if branch.is_empty() {
        return None;
    }
    write_cache(workspace_id, &branch);
    Some(branch)
}

/// The default branch the Compile action gates on and ships HEAD of, or `None`
/// for a workspace with no repository at all.
///
/// Not the same question as [`resolve_default_branch`], which inherits the git
/// client's `"main"` fallback when the lookup fails — right for the compiled
/// reader, wrong here. The blank `Default` workspace every admin-created org
/// starts with has no `.git`, so that fallback sent it down the git path and
/// every Compile answered 409 "could not resolve HEAD commit on main": the
/// workspace could never pick up an edit. With no repository, Compile takes
/// the snapshot path meant for exactly this case.
pub async fn compile_default_branch(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    workspace_path: &std::path::Path,
) -> Option<String> {
    if !oxy::github::default_git_client().is_git_repo(workspace_path) {
        return None;
    }
    resolve_default_branch(db, workspace_id).await
}

fn read_cache(workspace_id: Uuid) -> Option<String> {
    let guard = cache().read().ok()?;
    let entry = guard.get(&workspace_id)?;
    if entry.fetched_at.elapsed() > CACHE_TTL {
        return None;
    }
    Some(entry.name.clone())
}

fn write_cache(workspace_id: Uuid, branch: &str) {
    if let Ok(mut guard) = cache().write() {
        guard.insert(
            workspace_id,
            CachedBranch {
                name: branch.to_string(),
                fetched_at: Instant::now(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::compile_default_branch;
    use sea_orm::DatabaseConnection;
    use uuid::Uuid;

    #[tokio::test]
    async fn a_workspace_with_no_repository_compiles_as_a_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        // No `.git`: answered before any DB read, so a disconnected handle is fine.
        let branch =
            compile_default_branch(&DatabaseConnection::default(), Uuid::new_v4(), dir.path())
                .await;
        assert_eq!(branch, None);
    }
}
