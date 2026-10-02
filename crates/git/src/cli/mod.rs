pub mod auth;
pub mod branch;
pub mod clone;
pub mod commit;
pub mod config;
pub mod diff;
pub mod head;
pub mod path;
pub mod push_pull;
pub mod rebase;
mod redact;
pub mod repo;
pub mod run;
pub mod staging;
pub mod worktree;

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use oxy_shared::errors::OxyError;

use crate::cli::push_pull::PullOutcome;
use crate::client::GitClient;
use crate::types::{DirtyEntry, FileStatus, RecentCommit, ResetOutcome};

/// `GitClient` implementation that shells out to the system `git` binary.
#[derive(Debug, Clone, Default)]
pub struct CliGitClient;

impl CliGitClient {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl GitClient for CliGitClient {
    // ─── Clone / init ──────────────────────────────────────────────────

    async fn clone_or_init(
        &self,
        workspace_root: &Path,
        repo_url: Option<&str>,
        branch_name: &str,
        token: Option<&str>,
    ) -> Result<(), OxyError> {
        clone::clone_or_init(workspace_root, repo_url, branch_name, token).await
    }

    // ─── Repository helpers ────────────────────────────────────────────

    fn is_git_repo(&self, workspace_root: &Path) -> bool {
        repo::is_git_repo(workspace_root)
    }

    async fn ensure_initialized(&self, workspace_root: &Path) -> Result<(), OxyError> {
        repo::ensure_initialized(workspace_root).await
    }

    async fn has_remote(&self, workspace_root: &Path) -> bool {
        repo::has_remote(workspace_root).await
    }

    async fn get_default_branch(&self, workspace_root: &Path) -> String {
        repo::get_default_branch(workspace_root).await
    }

    // ─── Branch ────────────────────────────────────────────────────────

    fn validate_branch_name(&self, branch_name: &str) -> Result<(), OxyError> {
        branch::validate_branch_name(branch_name)
    }

    async fn get_current_branch(&self, workspace_root: &Path) -> Result<String, OxyError> {
        branch::get_current_branch(workspace_root).await
    }

    async fn head_state(&self, workspace_root: &Path) -> Result<head::HeadState, OxyError> {
        head::head_state(workspace_root).await
    }

    async fn fetch_branch_ref(
        &self,
        root: &Path,
        branch_name: &str,
        token: Option<&str>,
    ) -> Result<(), OxyError> {
        branch::fetch_branch_ref(root, branch_name, token).await
    }

    async fn list_branches_with_status(&self, workspace_root: &Path) -> Vec<(String, String)> {
        branch::list_branches_with_status(workspace_root).await
    }

    async fn list_branches_with_origin(
        &self,
        workspace_root: &Path,
        token: Option<&str>,
    ) -> Vec<crate::types::BranchInfo> {
        branch::list_branches_with_origin(workspace_root, token).await
    }

    async fn list_all_branches(
        &self,
        workspace_root: &Path,
        token: Option<&str>,
    ) -> Result<Vec<String>, OxyError> {
        branch::list_all_branches(workspace_root, token).await
    }

    async fn checkout_branch(
        &self,
        workspace_root: &Path,
        branch_name: &str,
        token: Option<&str>,
    ) -> Result<(), OxyError> {
        branch::checkout_branch(workspace_root, branch_name, token).await
    }

    async fn ensure_local_ref(
        &self,
        workspace_root: &Path,
        branch_name: &str,
        token: Option<&str>,
    ) -> Result<crate::types::LocalRefOrigin, OxyError> {
        branch::ensure_local_ref(workspace_root, branch_name, token).await
    }

    async fn delete_branch(
        &self,
        workspace_root: &Path,
        branch_name: &str,
    ) -> Result<(), OxyError> {
        branch::delete_branch(workspace_root, branch_name).await
    }

    // ─── Worktree ──────────────────────────────────────────────────────

    fn get_worktree_path(&self, workspace_root: &Path, branch_name: &str) -> Option<PathBuf> {
        worktree::get_worktree_path(workspace_root, branch_name)
    }

    async fn get_or_create_worktree(
        &self,
        workspace_root: &Path,
        branch_name: &str,
    ) -> Result<PathBuf, OxyError> {
        worktree::get_or_create_worktree(workspace_root, branch_name).await
    }

    async fn find_branch_checkout(
        &self,
        workspace_root: &Path,
        branch_name: &str,
    ) -> Result<Option<PathBuf>, OxyError> {
        worktree::find_branch_checkout(workspace_root, branch_name).await
    }

    // ─── Commit ────────────────────────────────────────────────────────

    async fn commit_changes(&self, root: &Path, message: &str) -> Result<String, OxyError> {
        commit::commit_changes(root, message).await
    }

    async fn get_head_commit_relative_date(&self, root: &Path) -> Option<String> {
        commit::get_head_commit_relative_date(root).await
    }

    async fn get_recent_commits(&self, root: &Path, n: usize, offset: usize) -> Vec<RecentCommit> {
        commit::get_recent_commits(root, n, offset).await
    }

    async fn get_commit_by_sha(&self, root: &Path, sha: &str) -> (String, String) {
        commit::get_commit_by_sha(root, sha).await
    }

    async fn get_branch_commit(&self, root: &Path, branch_name: &str) -> (String, String) {
        commit::get_branch_commit(root, branch_name).await
    }

    // ─── Diff ──────────────────────────────────────────────────────────

    async fn diff_numstat_summary(&self, repo_path: &Path) -> Result<Vec<FileStatus>, OxyError> {
        diff::numstat_summary(repo_path).await
    }

    async fn diff_numstat_ahead(&self, root: &Path) -> Result<Vec<FileStatus>, OxyError> {
        diff::numstat_ahead(root).await
    }

    async fn file_at_rev(
        &self,
        repo_path: &Path,
        file_path: &str,
        commit_ref: Option<&str>,
    ) -> Result<String, OxyError> {
        diff::file_at_rev(repo_path, file_path, commit_ref).await
    }

    // ─── Push / pull / remote ──────────────────────────────────────────

    async fn push_to_remote(&self, root: &Path, token: Option<&str>) -> Result<(), OxyError> {
        push_pull::push_to_remote(root, token).await
    }

    async fn force_push_to_remote(&self, root: &Path, token: Option<&str>) -> Result<(), OxyError> {
        push_pull::force_push_to_remote(root, token).await
    }

    async fn pull_from_remote(
        &self,
        worktree_root: &Path,
        branch_name: &str,
        token: Option<&str>,
    ) -> Result<PullOutcome, OxyError> {
        push_pull::pull_from_remote(worktree_root, branch_name, token).await
    }

    async fn fetch_remote_ref(
        &self,
        root: &Path,
        branch_name: &str,
        token: Option<&str>,
    ) -> Result<(), OxyError> {
        push_pull::fetch_remote_ref(root, branch_name, token).await
    }

    async fn is_behind_remote(&self, root: &Path, local_sha: &str, remote_sha: &str) -> bool {
        push_pull::is_behind_remote(root, local_sha, remote_sha).await
    }

    async fn get_ahead_behind_counts(
        &self,
        root: &Path,
        local_sha: &str,
        remote_sha: &str,
    ) -> (u64, u64) {
        push_pull::get_ahead_behind_counts(root, local_sha, remote_sha).await
    }

    async fn discard_all_changes(&self, root: &Path) -> Result<(), OxyError> {
        rebase::discard_all_changes(root).await
    }

    async fn get_tracking_ref_sha(&self, root: &Path, branch_name: &str) -> Option<String> {
        push_pull::get_tracking_ref_sha(root, branch_name).await
    }

    async fn get_remote_url(&self, workspace_root: &Path) -> Option<String> {
        push_pull::get_remote_url(workspace_root).await
    }

    // ─── Rebase / merge ────────────────────────────────────────────────

    async fn is_in_conflict(&self, root: &Path) -> bool {
        rebase::is_in_conflict(root)
    }

    async fn working_tree_status(&self, root: &Path) -> Result<Vec<DirtyEntry>, OxyError> {
        rebase::working_tree_status(root).await
    }

    async fn reset_to_commit(
        &self,
        root: &Path,
        commit_ref: &str,
        force: bool,
    ) -> Result<ResetOutcome, OxyError> {
        rebase::reset_to_commit(root, commit_ref, force).await
    }

    async fn abort_rebase(&self, root: &Path) -> Result<(), OxyError> {
        rebase::abort_rebase(root).await
    }

    async fn continue_rebase(&self, root: &Path) -> Result<(), OxyError> {
        rebase::continue_rebase(root).await
    }

    // ─── Conflict file staging ─────────────────────────────────────────

    async fn write_and_stage_file(
        &self,
        root: &Path,
        file_path: &str,
        content: &str,
    ) -> Result<(), OxyError> {
        staging::write_and_stage_file(root, file_path, content).await
    }

    async fn resolve_conflict_file(
        &self,
        root: &Path,
        file_path: &str,
        use_mine: bool,
    ) -> Result<(), OxyError> {
        staging::resolve_conflict_file(root, file_path, use_mine).await
    }

    async fn unresolve_conflict_file(&self, root: &Path, file_path: &str) -> Result<(), OxyError> {
        staging::unresolve_conflict_file(root, file_path).await
    }
}
