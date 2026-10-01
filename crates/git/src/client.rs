use std::path::{Path, PathBuf};

use async_trait::async_trait;
use oxy_shared::errors::OxyError;

use crate::cli::push_pull::PullOutcome;
use crate::types::{DirtyEntry, FileStatus, RecentCommit, ResetOutcome};

/// Unified git operations surface.
///
/// All handlers and services talk to this trait. The only implementation is
/// [`crate::cli::CliGitClient`], which shells out to the system `git` binary.
///
/// Methods that accept `token: Option<&str>` convert to `Auth::Bearer`
/// internally and inject the credential via `http.extraHeader`.
#[async_trait]
pub trait GitClient: Send + Sync {
    // ─── Clone / init ──────────────────────────────────────────────────

    /// Clone `repo_url` into `workspace_root`, or `git init` if `repo_url`
    /// is `None`.  No-op if a `.git` already exists.
    async fn clone_or_init(
        &self,
        workspace_root: &Path,
        repo_url: Option<&str>,
        branch: &str,
        token: Option<&str>,
    ) -> Result<(), OxyError>;

    // ─── Repository helpers ────────────────────────────────────────────

    fn is_git_repo(&self, workspace_root: &Path) -> bool;

    async fn ensure_initialized(&self, workspace_root: &Path) -> Result<(), OxyError>;

    async fn has_remote(&self, workspace_root: &Path) -> bool;

    async fn get_default_branch(&self, workspace_root: &Path) -> String;

    // ─── Branch ────────────────────────────────────────────────────────

    fn validate_branch_name(&self, branch: &str) -> Result<(), OxyError>;

    async fn get_current_branch(&self, workspace_root: &Path) -> Result<String, OxyError>;

    async fn fetch_branch_ref(
        &self,
        root: &Path,
        branch: &str,
        token: Option<&str>,
    ) -> Result<(), OxyError>;

    async fn list_branches_with_status(&self, workspace_root: &Path) -> Vec<(String, String)>;

    /// Lists every branch the user can switch to — both local and
    /// remote-only — with their origin and sync status. Performs a
    /// best-effort fetch first; failures are swallowed and the cached refs
    /// are returned.
    async fn list_branches_with_origin(
        &self,
        workspace_root: &Path,
        token: Option<&str>,
    ) -> Vec<crate::types::BranchInfo>;

    async fn list_all_branches(
        &self,
        workspace_root: &Path,
        token: Option<&str>,
    ) -> Result<Vec<String>, OxyError>;

    async fn checkout_branch(
        &self,
        workspace_root: &Path,
        branch: &str,
        token: Option<&str>,
    ) -> Result<(), OxyError>;

    /// Ensures `branch` exists locally without modifying the current worktree's
    /// HEAD or working files, and reports whether it also exists on origin.
    /// See `branch::ensure_local_ref`.
    async fn ensure_local_ref(
        &self,
        workspace_root: &Path,
        branch: &str,
        token: Option<&str>,
    ) -> Result<crate::types::LocalRefOrigin, OxyError>;

    async fn delete_branch(&self, workspace_root: &Path, branch: &str) -> Result<(), OxyError>;

    // ─── Worktree ──────────────────────────────────────────────────────

    fn get_worktree_path(&self, workspace_root: &Path, branch: &str) -> Option<PathBuf>;

    async fn get_or_create_worktree(
        &self,
        workspace_root: &Path,
        branch: &str,
    ) -> Result<PathBuf, OxyError>;

    /// Finds the worktree — if any — where `branch` is currently checked out,
    /// including the repo's main working copy. See
    /// [`crate::cli::worktree::find_branch_checkout`] for the full contract;
    /// this is the shared helper callers use to resolve a branch to the
    /// checkout that actually holds it, rather than assuming it, so
    /// `get_or_create_worktree` and workspace-path resolution never disagree.
    async fn find_branch_checkout(
        &self,
        workspace_root: &Path,
        branch: &str,
    ) -> Result<Option<PathBuf>, OxyError>;

    // ─── Commit ────────────────────────────────────────────────────────

    async fn commit_changes(&self, root: &Path, message: &str) -> Result<String, OxyError>;

    async fn get_head_commit_relative_date(&self, root: &Path) -> Option<String>;

    async fn get_recent_commits(&self, root: &Path, n: usize, offset: usize) -> Vec<RecentCommit>;

    async fn get_commit_by_sha(&self, root: &Path, sha: &str) -> (String, String);

    async fn get_branch_commit(&self, root: &Path, branch: &str) -> (String, String);

    // ─── Diff ──────────────────────────────────────────────────────────

    async fn diff_numstat_summary(&self, repo_path: &Path) -> Result<Vec<FileStatus>, OxyError>;

    async fn diff_numstat_ahead(&self, root: &Path) -> Result<Vec<FileStatus>, OxyError>;

    async fn file_at_rev(
        &self,
        repo_path: &Path,
        file_path: &str,
        commit: Option<&str>,
    ) -> Result<String, OxyError>;

    // ─── Push / pull / remote ──────────────────────────────────────────

    async fn push_to_remote(&self, root: &Path, token: Option<&str>) -> Result<(), OxyError>;

    async fn force_push_to_remote(&self, root: &Path, token: Option<&str>) -> Result<(), OxyError>;

    async fn pull_from_remote(
        &self,
        worktree_root: &Path,
        branch: &str,
        token: Option<&str>,
    ) -> Result<PullOutcome, OxyError>;

    /// Update the remote-tracking ref for `branch` without touching the
    /// local branch. Use this for a "Fetch" CTA — see `pull_from_remote`
    /// when you also want to integrate the fetched commits.
    async fn fetch_remote_ref(
        &self,
        root: &Path,
        branch: &str,
        token: Option<&str>,
    ) -> Result<(), OxyError>;

    async fn is_behind_remote(&self, root: &Path, local_sha: &str, remote_sha: &str) -> bool;

    /// Returns `(ahead_count, behind_count)` of `local_sha` relative to
    /// `remote_sha`. Used to drive the IDE's status pill and to enable/disable
    /// Pull/Push buttons independently. Cheap: one `git rev-list --left-right
    /// --count` invocation.
    async fn get_ahead_behind_counts(
        &self,
        root: &Path,
        local_sha: &str,
        remote_sha: &str,
    ) -> (u64, u64);

    /// Discard ALL working-tree changes (tracked + untracked, including
    /// directories) by running `git reset --hard HEAD && git clean -fd`.
    /// Destructive — gated by admin capability at the API layer.
    async fn discard_all_changes(&self, root: &Path) -> Result<(), OxyError>;

    async fn get_tracking_ref_sha(&self, root: &Path, branch: &str) -> Option<String>;

    async fn get_remote_url(&self, workspace_root: &Path) -> Option<String>;

    // ─── Rebase / merge ────────────────────────────────────────────────

    async fn is_in_conflict(&self, root: &Path) -> bool;

    /// List working-tree entries that would be discarded by a hard reset.
    /// Returned entries cover modified, staged, untracked, deleted, and
    /// conflicted files. Empty vec ⇒ tree is clean.
    async fn working_tree_status(&self, root: &Path) -> Result<Vec<DirtyEntry>, OxyError>;

    /// Restore the working tree to `commit` and create a new "Restore to …"
    /// commit on top of the current HEAD.
    ///
    /// If `force` is false and the working tree has any uncommitted changes
    /// (modified, staged, untracked, deleted, or conflicted), the call is a
    /// no-op and returns [`ResetOutcome::Dirty`] with the file list so the UI
    /// can prompt the user before discarding work. With `force = true`, the
    /// working tree is fully replaced (tracked + untracked) to match `commit`.
    async fn reset_to_commit(
        &self,
        root: &Path,
        commit: &str,
        force: bool,
    ) -> Result<ResetOutcome, OxyError>;

    async fn abort_rebase(&self, root: &Path) -> Result<(), OxyError>;

    async fn continue_rebase(&self, root: &Path) -> Result<(), OxyError>;

    // ─── Conflict file staging ─────────────────────────────────────────

    async fn write_and_stage_file(
        &self,
        root: &Path,
        file_path: &str,
        content: &str,
    ) -> Result<(), OxyError>;

    async fn resolve_conflict_file(
        &self,
        root: &Path,
        file_path: &str,
        use_mine: bool,
    ) -> Result<(), OxyError>;

    async fn unresolve_conflict_file(&self, root: &Path, file_path: &str) -> Result<(), OxyError>;
}
