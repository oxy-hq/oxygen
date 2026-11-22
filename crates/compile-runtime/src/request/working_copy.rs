//! The branch as this node's working copy has it: the path a compile took
//! before a commit could be fetched, kept for what GitHub cannot serve — a
//! branch that was never pushed, a workspace with no remote or no connection.
//!
//! Only called on a process that owns working copies. It goes when the IDE
//! does (`internal-docs/factory-retirement.md`, phase 5).

use oxy_git::GitClient;

use super::Refusal;
use super::from_git::NotFromGit;

/// The branch's head commit in the working copy, after making sure its
/// worktree exists on this node so the compile worker can read it.
///
/// Refuses, rather than guesses, in the cases where the SHA would not describe
/// what gets compiled: an unknown branch (`get_or_create_worktree` would
/// otherwise CREATE it from main's HEAD), a workspace with no repository, and
/// a worktree with uncommitted edits (the revision would carry the branch
/// head's SHA but content that is on no commit).
pub(super) async fn branch_head(
    workspace: &entity::workspaces::Model,
    branch: &str,
    why_not_git: &NotFromGit,
) -> Result<String, Refusal> {
    let root = workspace
        .path
        .as_deref()
        .map(std::path::PathBuf::from)
        .ok_or_else(|| {
            Refusal::Conflict(
                "workspace has no on-disk path — a branch cannot be compiled here".to_string(),
            )
        })?;
    let git = oxy::github::default_git_client();
    let (sha, _subject) = git.get_branch_commit(&root, branch).await;
    if sha.is_empty() {
        return Err(Refusal::UnknownBranch(unknown_branch(
            workspace.id,
            branch,
            why_not_git,
        )));
    }
    if let Err(e) = git.get_or_create_worktree(&root, branch).await {
        return Err(worktree_add_conflict(&git, &root, branch, e).await);
    }
    let worktree = oxy::adapters::workspace::effective_workspace_path(workspace, Some(branch))
        .await
        .map_err(Refusal::internal)?;
    if !oxy_git::cli::worktree::is_worktree_clean(&worktree)
        .await
        .unwrap_or(false)
    {
        return Err(Refusal::Conflict(format!(
            "branch {branch:?} has uncommitted changes in the workspace — commit them so the \
             staging revision matches a commit"
        )));
    }
    Ok(sha)
}

/// The branch is in neither place it could be. When GitHub was asked, say so:
/// "does not exist in the workspace" alone sends a person looking for a typo
/// in a branch that is simply not pushed.
fn unknown_branch(workspace_id: uuid::Uuid, branch: &str, why_not_git: &NotFromGit) -> String {
    match why_not_git {
        NotFromGit::BranchNotFound { owner, repo, .. } => format!(
            "branch {branch:?} is not on GitHub ({owner}/{repo}) and not in the working copy \
             of workspace {workspace_id} — push it first"
        ),
        _ => format!("branch {branch:?} does not exist in workspace {workspace_id}"),
    }
}

/// Turns a `get_or_create_worktree` failure into a refusal, not a bare 500.
///
/// `get_or_create_worktree` already reuses a branch's existing checkout when
/// one exists (see `oxy_git::cli::worktree::find_branch_checkout`), so a
/// failure here means something changed the state out from under that check
/// — most plausibly a race where another process checked the branch out
/// between the two calls. Either way this is a conflict the caller can
/// retry or resolve, not a server bug: name the checkout that is in the way
/// when we can find one, and fall back to the raw git error otherwise.
async fn worktree_add_conflict(
    git: &impl GitClient,
    root: &std::path::Path,
    branch: &str,
    err: oxy_shared::errors::OxyError,
) -> Refusal {
    tracing::error!(%branch, error = %err, "compile request: worktree add failed");
    let message = match git.find_branch_checkout(root, branch).await {
        Ok(Some(path)) => format!(
            "branch {branch:?} is checked out at {}; staging compile cannot add a second \
             worktree for it",
            path.display()
        ),
        _ => format!("could not prepare a worktree for branch {branch:?}: {err}"),
    };
    Refusal::Conflict(message)
}
