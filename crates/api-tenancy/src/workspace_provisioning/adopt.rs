//! Connecting a repository to the org's one workspace.
//!
//! Orgs created before 2026-11-13 carry a blank `Default` workspace with no
//! remote (a new org has none; the import creates it). The GitHub import used to
//! create a second workspace beside such a blank one ("Default 2"),
//! which is how orgs came to hold several. Now the import *adopts* the org's
//! workspace instead: the repository columns are recorded on the existing row,
//! its working copy is set aside, and the clone lands where the blank one was.
//! The workspace id does not change, so everything already keyed by it (the
//! Airhouse tenant, secrets, published apps, the health schedule) carries over.

use std::path::{Path, PathBuf};

use entity::prelude::Workspaces;
use entity::workspaces::{self, WorkspaceStatus};
use oxy::adapters::workspace::workspace_root_path;
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, Condition, ConnectionTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::{ProvisionError, RepositoryOrigin, org_workspace, query_failed};

/// Where a GitHub import for an org lands.
#[derive(Debug, PartialEq, Eq)]
pub enum ImportTarget {
    /// The org has no workspace (an org-less local row, or one whose workspace
    /// was deleted): create it.
    New,
    /// The org's workspace has no repository yet, or its last import never
    /// produced one (the clone failed, or found no Oxygen project): connect
    /// this one to it.
    Adopt(Uuid),
}

/// Decide where an import for `org_id` lands. An org whose workspace already
/// has a working repository (or an import in flight) is refused: changing the
/// repository is not an import.
pub async fn import_target<C: ConnectionTrait>(
    conn: &C,
    org_id: Uuid,
) -> Result<ImportTarget, ProvisionError> {
    let existing = org_workspace(conn, org_id).await?;
    let now = chrono::Utc::now().fixed_offset();
    target_for(existing.map(|ws| (ws.id, adoptable(&ws, now))))
}

/// Statuses an import leaves behind when it never produced a working copy,
/// so the next import may take the workspace over again. Without this, one
/// wrong branch or subdirectory would leave the org's only workspace refusing
/// every import.
const FAILED_IMPORT: [WorkspaceStatus; 2] =
    [WorkspaceStatus::Failed, WorkspaceStatus::NotOxyProject];

/// How long a `Cloning` row may sit untouched before its import counts as
/// failed. The clone runs on the request's pod; one that dies mid-clone never
/// writes a final status, and the row would otherwise refuse every import.
/// A live clone touches the row every [`HEARTBEAT`] ([`heartbeat`]), so only
/// a dead one goes stale, and a slow clone is never taken over.
pub const STALE_CLONE: chrono::Duration = chrono::Duration::hours(1);

/// How often a running clone touches its row: a twelfth of [`STALE_CLONE`],
/// so several missed beats in a row still don't make a live clone stale.
pub const HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// The longest a clone may run. The heartbeat keeps a live clone from going
/// stale, so without a bound a hung `git clone` (a half-open connection, a
/// server that stops sending) would hold the org's only workspace in
/// `Cloning` for as long as the pod lives. Past this the clone is abandoned
/// and the row settles `Failed`, which the next import may take over.
pub const MAX_CLONE: std::time::Duration = std::time::Duration::from_secs(2 * 60 * 60);

type Timestamp = chrono::DateTime<chrono::FixedOffset>;

fn adoptable(ws: &workspaces::Model, now: Timestamp) -> bool {
    adoptable_state(ws.git_remote_url.is_some(), &ws.status, ws.updated_at, now)
}

/// No repository yet, an import that failed, or one whose clone went stale.
fn adoptable_state(
    has_remote: bool,
    status: &WorkspaceStatus,
    updated_at: Timestamp,
    now: Timestamp,
) -> bool {
    !has_remote
        || FAILED_IMPORT.contains(status)
        || (*status == WorkspaceStatus::Cloning && updated_at < now - STALE_CLONE)
}

/// `existing` is the org's workspace as `(id, adoptable)`.
fn target_for(existing: Option<(Uuid, bool)>) -> Result<ImportTarget, ProvisionError> {
    match existing {
        None => Ok(ImportTarget::New),
        Some((id, true)) => Ok(ImportTarget::Adopt(id)),
        Some((_, false)) => Err(ProvisionError::OrgHasWorkspace),
    }
}

/// What [`claim`] replaced, so [`release`] can put it back.
pub struct Claim {
    workspace_id: Uuid,
    previous: workspaces::Model,
}

impl Claim {
    pub fn workspace_id(&self) -> Uuid {
        self.workspace_id
    }
}

/// Record `origin` on `workspace_id`, point its row at `project_dir` and mark
/// it Cloning. The update only matches a row that is still adoptable (no
/// remote, or a failed import), and the claim itself moves it to Cloning, so
/// of two concurrent imports exactly one claims the workspace; the other is
/// refused as [`ProvisionError::OrgHasWorkspace`]. `name`, when given, renames
/// the workspace; otherwise it keeps its name.
pub async fn claim<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    project_dir: &Path,
    origin: RepositoryOrigin,
    name: Option<&str>,
) -> Result<Claim, ProvisionError> {
    let previous = Workspaces::find_by_id(workspace_id)
        .one(conn)
        .await
        .map_err(query_failed)?
        .ok_or(ProvisionError::OrgHasWorkspace)?;
    let now = chrono::Utc::now().fixed_offset();
    let stale_before = now - STALE_CLONE;
    let (namespace, remote, branch, subdir) = origin.into_columns();
    let mut update = Workspaces::update_many()
        .col_expr(workspaces::Column::GitNamespaceId, Expr::value(namespace))
        .col_expr(workspaces::Column::GitRemoteUrl, Expr::value(remote))
        .col_expr(workspaces::Column::DefaultBranch, Expr::value(branch))
        .col_expr(workspaces::Column::RepoSubdir, Expr::value(subdir))
        .col_expr(
            workspaces::Column::Path,
            Expr::value(project_dir.to_string_lossy().to_string()),
        )
        .col_expr(
            workspaces::Column::Status,
            Expr::value(WorkspaceStatus::Cloning),
        )
        .col_expr(
            workspaces::Column::Error,
            Expr::value(Option::<String>::None),
        )
        .col_expr(workspaces::Column::UpdatedAt, Expr::value(now));
    if let Some(name) = name {
        update = update.col_expr(workspaces::Column::Name, Expr::value(name.to_string()));
    }
    let result = update
        .filter(workspaces::Column::Id.eq(workspace_id))
        .filter(
            Condition::any()
                .add(workspaces::Column::GitRemoteUrl.is_null())
                .add(workspaces::Column::Status.is_in(FAILED_IMPORT))
                .add(
                    Condition::all()
                        .add(workspaces::Column::Status.eq(WorkspaceStatus::Cloning))
                        .add(workspaces::Column::UpdatedAt.lt(stale_before)),
                ),
        )
        .exec(conn)
        .await
        .map_err(query_failed)?;
    if result.rows_affected == 0 {
        return Err(ProvisionError::OrgHasWorkspace);
    }
    Ok(Claim {
        workspace_id,
        previous,
    })
}

/// Write a finished clone's status, only if the row is still the attempt that
/// started it: `Cloning`, with the remote that attempt recorded. Only an import
/// writes `git_remote_url`, so a rename or an org transfer during the clone
/// (which do move `updated_at`) cannot drop its result; a stale takeover
/// ([`STALE_CLONE`]) by another repository makes the old clone write nothing,
/// so it cannot mark the newer attempt Ready or Failed. A takeover by the same
/// remote cannot overlap a live clone, because a live clone keeps the row
/// fresh ([`heartbeat`]); only a dead one is taken over. Returns whether it
/// wrote.
pub async fn settle<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    remote_url: &str,
    status: WorkspaceStatus,
    error: Option<String>,
) -> Result<bool, ProvisionError> {
    let result = Workspaces::update_many()
        .col_expr(workspaces::Column::Status, Expr::value(status))
        .col_expr(workspaces::Column::Error, Expr::value(error))
        .col_expr(
            workspaces::Column::UpdatedAt,
            Expr::value(chrono::Utc::now().fixed_offset()),
        )
        .filter(workspaces::Column::Id.eq(workspace_id))
        .filter(workspaces::Column::Status.eq(WorkspaceStatus::Cloning))
        .filter(workspaces::Column::GitRemoteUrl.eq(remote_url))
        .exec(conn)
        .await
        .map_err(query_failed)?;
    Ok(result.rows_affected == 1)
}

/// Touch a running clone's row, so it does not read as stale while the clone
/// is alive. Matches the same attempt [`settle`] does; a row that has moved on
/// is left alone.
pub async fn heartbeat<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    remote_url: &str,
) -> Result<(), ProvisionError> {
    Workspaces::update_many()
        .col_expr(
            workspaces::Column::UpdatedAt,
            Expr::value(chrono::Utc::now().fixed_offset()),
        )
        .filter(workspaces::Column::Id.eq(workspace_id))
        .filter(workspaces::Column::Status.eq(WorkspaceStatus::Cloning))
        .filter(workspaces::Column::GitRemoteUrl.eq(remote_url))
        .exec(conn)
        .await
        .map_err(query_failed)?;
    Ok(())
}

/// Undo a [`claim`] whose clone never started: the row gets back its name,
/// path, status and empty repository columns, so the import can be retried.
pub async fn release<C: ConnectionTrait>(conn: &C, claim: Claim) -> Result<(), ProvisionError> {
    let p = claim.previous;
    Workspaces::update_many()
        .col_expr(
            workspaces::Column::GitNamespaceId,
            Expr::value(p.git_namespace_id),
        )
        .col_expr(
            workspaces::Column::GitRemoteUrl,
            Expr::value(p.git_remote_url),
        )
        .col_expr(
            workspaces::Column::DefaultBranch,
            Expr::value(p.default_branch),
        )
        .col_expr(workspaces::Column::RepoSubdir, Expr::value(p.repo_subdir))
        .col_expr(workspaces::Column::Path, Expr::value(p.path))
        .col_expr(workspaces::Column::Status, Expr::value(p.status))
        .col_expr(workspaces::Column::Error, Expr::value(p.error))
        .col_expr(workspaces::Column::Name, Expr::value(p.name))
        .filter(workspaces::Column::Id.eq(claim.workspace_id))
        .exec(conn)
        .await
        .map_err(query_failed)?;
    Ok(())
}

/// Move the workspace's current working copy out of the way, so the clone can
/// land at its root. Kept beside it (`<id>.before-import-<unix secs>`), never
/// deleted: a blank workspace may still hold edits nobody pushed anywhere.
/// `None` when there was nothing on disk.
pub fn set_aside_working_copy(workspace_id: Uuid) -> Result<Option<PathBuf>, ProvisionError> {
    let root = workspace_root_path(workspace_id);
    if !root.exists() {
        return Ok(None);
    }
    let stamp = chrono::Utc::now().timestamp();
    let aside = root.with_file_name(format!("{workspace_id}.before-import-{stamp}"));
    std::fs::rename(&root, &aside).map_err(|e| {
        ProvisionError::Filesystem(format!(
            "Failed to set aside the working copy of workspace {workspace_id}: {e}"
        ))
    })?;
    tracing::info!(%workspace_id, aside = ?aside, "set aside working copy before connecting a repository");
    Ok(Some(aside))
}

/// Undo [`set_aside_working_copy`] for an import abandoned before its clone:
/// whatever was made at the root since is removed, and the set-aside copy
/// goes back. Best-effort, and logged: the copy is never deleted either way.
pub fn restore_working_copy(workspace_id: Uuid, aside: Option<PathBuf>) {
    let root = workspace_root_path(workspace_id);
    if root.exists()
        && let Err(e) = std::fs::remove_dir_all(&root)
    {
        tracing::warn!(%workspace_id, error = %e, "could not clear the root of an abandoned import");
        return;
    }
    if let Some(aside) = aside
        && let Err(e) = std::fs::rename(&aside, &root)
    {
        tracing::warn!(%workspace_id, aside = ?aside, error = %e, "could not put the set-aside working copy back");
    }
}

#[cfg(test)]
#[path = "adopt_tests.rs"]
mod tests;
