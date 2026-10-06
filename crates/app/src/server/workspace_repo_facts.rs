//! What Postgres remembers about the repository behind a workspace: its
//! default branch (`workspaces.default_branch`) and where the workspace root
//! sits inside it (`workspaces.repo_subdir`).
//!
//! Both are facts about a checkout — `origin/HEAD` in its `.git`, the
//! subdirectory folded into `workspaces.path` — so until they are columns,
//! only the node holding that checkout can answer either. GitHub onboarding
//! records both when it creates a workspace; [`record_from_checkout`] is the
//! backfill for rows that predate the columns, and the correction when a
//! repository's default branch changes.
//!
//! One-way on purpose. Local git stays the authority wherever there is a
//! checkout, and these columns are what a process *without* one reads
//! ([`stored_default_branch`]). Such a process never writes them: it has no
//! checkout to read, so whatever it wrote would be a guess.

use std::path::Path;

use entity::workspaces::{self, WorkspaceStatus};
use oxy_git::cli::repo::{is_git_repo, remote_default_branch, subdir_in_repo};
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

/// The default branch recorded on `row`, if one has been.
pub fn stored_default_branch(row: &workspaces::Model) -> Option<String> {
    row.default_branch.clone().filter(|b| !b.is_empty())
}

/// The workspace's subdirectory in its repository as recorded on `row`.
/// `None` is the repository root: NULL and the empty string mean the same.
pub fn stored_repo_subdir(row: &workspaces::Model) -> Option<&str> {
    row.repo_subdir.as_deref().filter(|s| !s.is_empty())
}

/// Bring `row`'s two columns in line with the checkout at `workspace_path`,
/// writing only what differs. Returns whether anything was written.
///
/// `resolved_branch` is what the git client just answered for this checkout.
/// It is compared, never stored: it may be the process-wide
/// `GIT_DEFAULT_BRANCH` override or the client's `"main"` guess, and neither
/// is a fact about this repository. What gets stored is `origin/HEAD`.
///
/// Best-effort: a failed write is logged and reported as "nothing written",
/// because the caller is answering a read and must not fail on bookkeeping.
pub async fn record_from_checkout(
    db: &DatabaseConnection,
    row: &workspaces::Model,
    workspace_path: &Path,
    resolved_branch: &str,
) -> bool {
    if !has_a_checkout_to_read(row, workspace_path) {
        return false;
    }
    let branch = branch_to_record(row, workspace_path, resolved_branch).await;
    let subdir = subdir_in_repo(workspace_path);
    let subdir_differs = stored_repo_subdir(row) != subdir.as_deref();
    if branch.is_none() && !subdir_differs {
        return false;
    }

    let mut update = workspaces::Entity::update_many().filter(workspaces::Column::Id.eq(row.id));
    if let Some(branch) = &branch {
        update = update.col_expr(
            workspaces::Column::DefaultBranch,
            Expr::value(branch.clone()),
        );
    }
    if subdir_differs {
        update = update.col_expr(workspaces::Column::RepoSubdir, Expr::value(subdir.clone()));
    }
    match update.exec(db).await {
        Ok(_) => {
            tracing::info!(
                workspace_id = %row.id,
                default_branch = ?branch,
                repo_subdir = ?subdir,
                "recorded a workspace's repository facts from its checkout"
            );
            true
        }
        Err(e) => {
            tracing::warn!(workspace_id = %row.id, error = %e, "recording repository facts failed");
            false
        }
    }
}

/// Whether this process may read `row`'s repository facts off disk.
///
/// Four conditions, each closing a way to record something untrue: the process
/// owns working copies at all (a serve or worker pod does not); the workspace
/// has a remote (one without keeps both columns NULL); its clone has landed
/// (while it is `Cloning` the directory exists with no `.git` of its own, and
/// git discovery would walk up into whatever repository encloses the state
/// directory); and the checkout is actually there.
fn has_a_checkout_to_read(row: &workspaces::Model, workspace_path: &Path) -> bool {
    oxy::workspace_fs_probe::process_owns_workspace_files()
        && row.git_remote_url.is_some()
        && row.status == WorkspaceStatus::Ready
        && workspace_path.is_dir()
        && is_git_repo(workspace_path)
}

/// The branch to store, or `None` when the row already agrees or git cannot
/// name one. Asks git only when the row disagrees with what was just
/// resolved, so a row that is already right costs no subprocess.
async fn branch_to_record(
    row: &workspaces::Model,
    workspace_path: &Path,
    resolved_branch: &str,
) -> Option<String> {
    if row.default_branch.as_deref() == Some(resolved_branch) {
        return None;
    }
    let branch = remote_default_branch(workspace_path)
        .await
        .filter(|b| !b.is_empty())?;
    (row.default_branch.as_deref() != Some(branch.as_str())).then_some(branch)
}
