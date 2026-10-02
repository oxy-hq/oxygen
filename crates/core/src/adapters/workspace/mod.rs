pub mod builder;
pub mod manager;

use std::path::{Path, PathBuf};

use sea_orm::EntityTrait;
use uuid::Uuid;

use crate::config::resolve_local_workspace_path;
use crate::database::client::establish_connection;
use crate::github::default_git_client;
use crate::state_dir::get_state_dir;
use oxy_git::GitClient;
use oxy_git::cli::repo::find_git_root;
use oxy_shared::errors::OxyError;

/// Canonical on-disk root for a workspace: `<state_dir>/workspaces/<workspace_id>`.
///
/// Single generation point for a workspace's filesystem location. The value is
/// stored in `workspaces.path` at registration time; no other code should
/// synthesize this path.
pub fn workspace_root_path(workspace_id: Uuid) -> PathBuf {
    get_state_dir()
        .join("workspaces")
        .join(workspace_id.to_string())
}

/// Compute the effective workspace path for a given branch.
///
/// Starts from `workspace_row.path` (the root). When `branch` is non-empty,
/// valid, and not the repo's default branch, overlays the matching worktree
/// when it exists on disk. Falls back to the root otherwise.
///
/// One value is not a branch name: the label the server reports for a detached
/// HEAD (`HEAD@<sha>`, owned by `oxy_git::cli::head`). A detached working copy
/// has no branch, so that label names the root as it is — exactly what no
/// `branch` names — and only while the root really is detached at that commit.
///
/// The only place the backend turns `(workspace, branch)` into a filesystem
/// path — both `workspace_middleware` and `resolve_workspace_path` funnel
/// through here so branch/worktree semantics stay consistent.
pub async fn effective_workspace_path(
    workspace_row: &entity::workspaces::Model,
    branch: Option<&str>,
) -> Result<PathBuf, OxyError> {
    let root = workspace_row
        .path
        .clone()
        .map(PathBuf::from)
        .ok_or_else(|| {
            OxyError::ConfigurationError(format!(
                "Workspace {} has no path configured",
                workspace_row.id
            ))
        })?;

    let Some(branch) = branch.map(str::trim).filter(|b| !b.is_empty()) else {
        crate::workspace_fs_probe::note_workspace_path_resolved(Some(workspace_row.id), &root);
        return Ok(root);
    };

    let resolved = if oxy_git::is_detached_label(branch) {
        // Checked against the working copy, never looked up: not as a ref, not
        // as a `.worktrees/` directory. A label for any commit but the one the
        // root is detached at — stale or made up — is refused.
        oxy_git::cli::head::verify_detached_label(&root, branch).await?;
        root
    } else {
        default_git_client().validate_branch_name(branch)?;
        branch_checkout(root, branch).await
    };
    crate::workspace_fs_probe::note_workspace_path_resolved(Some(workspace_row.id), &resolved);
    Ok(resolved)
}

/// Where the (already validated) `branch` is checked out: the root for the
/// default branch, its worktree otherwise, and the root when it has none.
async fn branch_checkout(root: PathBuf, branch: &str) -> PathBuf {
    let git = default_git_client();
    if branch == git.get_default_branch(&root).await {
        return root;
    }
    // The `.worktrees/<branch>` disk check answers the common case without
    // spawning git on every request. On a miss, ask git where the branch
    // is checked out — the same lookup `get_or_create_worktree` uses, so a
    // branch held by the repo's main working copy resolves there. A failed
    // lookup (no repository) keeps the old fallback to the root.
    let checkout = match git.get_worktree_path(&root, branch) {
        Some(worktree) => Some(worktree),
        None => git
            .find_branch_checkout(&root, branch)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(%branch, error = %e, "could not list worktrees; using the workspace root");
                None
            }),
    };
    match checkout {
        Some(worktree) => worktree_project_path(&root, worktree),
        None => root,
    }
}

/// Re-apply the workspace's in-repo subdirectory onto a worktree path.
///
/// A git worktree is a full checkout of the *repository root*. When a workspace
/// lives in a subdirectory of the repo (`workspace.path` is `…/<repo>/sub/dir`),
/// the project — including `config.yml` — lives at `<worktree>/sub/dir`, not at
/// the worktree root. Without this, branch switching on a subdirectory workspace
/// resolves config from `<worktree>/config.yml`, which does not exist and fails
/// with "No such file or directory".
///
/// Returns `worktree` unchanged when the workspace is at the repository root (or
/// no git root can be found), preserving the non-subdirectory behaviour.
fn worktree_project_path(root: &Path, worktree: PathBuf) -> PathBuf {
    let Some(git_root) = find_git_root(root) else {
        return worktree;
    };
    match root.strip_prefix(&git_root) {
        Ok(subdir) if !subdir.as_os_str().is_empty() => worktree.join(subdir),
        _ => worktree,
    }
}

pub async fn resolve_workspace_path(workspace_id: Uuid) -> Result<PathBuf, OxyError> {
    if workspace_id.is_nil() {
        // The nil workspace is local mode by definition; it always owns its files.
        return resolve_local_workspace_path().map_err(|e| {
            OxyError::ConfigurationError(format!("Failed to resolve local project path: {}", e))
        });
    }

    let conn = establish_connection().await?;
    let workspace = entity::prelude::Workspaces::find_by_id(workspace_id)
        .one(&conn)
        .await
        .map_err(|e| OxyError::DBError(e.to_string()))?
        .ok_or_else(|| OxyError::DBError(format!("Workspace {} not found", workspace_id)))?;

    effective_workspace_path(&workspace, None).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn worktree_project_path_appends_subdirectory() {
        // Repo root holds `.git`; the workspace lives in `packages/app`.
        let repo = TempDir::new().unwrap();
        fs::create_dir(repo.path().join(".git")).unwrap();
        let workspace_root = repo.path().join("packages").join("app");
        fs::create_dir_all(&workspace_root).unwrap();

        let worktree = workspace_root.join(".worktrees").join("feature");
        let resolved = worktree_project_path(&workspace_root, worktree.clone());

        // Config must resolve under `<worktree>/packages/app`, since a worktree
        // is a full checkout of the repository root.
        assert_eq!(resolved, worktree.join("packages").join("app"));
    }

    #[test]
    fn worktree_project_path_unchanged_at_repo_root() {
        // Workspace is the repo root itself — no subdirectory to re-apply.
        let repo = TempDir::new().unwrap();
        fs::create_dir(repo.path().join(".git")).unwrap();

        let worktree = repo.path().join(".worktrees").join("feature");
        let resolved = worktree_project_path(repo.path(), worktree.clone());

        assert_eq!(resolved, worktree);
    }

    #[test]
    fn worktree_project_path_unchanged_without_git_root() {
        // No `.git` anywhere up the tree — fall back to the worktree as-is.
        let dir = TempDir::new().unwrap();
        let workspace_root = dir.path().join("sub");
        fs::create_dir_all(&workspace_root).unwrap();

        let worktree = workspace_root.join(".worktrees").join("feature");
        let resolved = worktree_project_path(&workspace_root, worktree.clone());

        assert_eq!(resolved, worktree);
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A workspace whose root is a repo on `main` with two commits. Returns
    /// `(dir, row, first_sha)`; the root is canonical so paths compare equal.
    fn workspace_repo() -> (TempDir, entity::workspaces::Model, String) {
        let tmp = TempDir::new().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "user.email", "t@example.com"]);
        git(&root, &["config", "user.name", "Test"]);
        fs::write(root.join("config.yml"), "one").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-qm", "first"]);
        let first = git(&root, &["rev-parse", "--short", "HEAD"]);
        fs::write(root.join("config.yml"), "two").unwrap();
        git(&root, &["commit", "-qam", "second"]);
        let now = chrono::Utc::now().into();
        let row = entity::workspaces::Model {
            id: Uuid::new_v4(),
            name: "ws".to_string(),
            git_namespace_id: None,
            git_remote_url: None,
            created_at: now,
            updated_at: now,
            path: Some(root.to_string_lossy().into_owned()),
            last_opened_at: None,
            created_by: None,
            org_id: None,
            status: entity::workspaces::WorkspaceStatus::Ready,
            error: None,
            monthly_vlm_budget_micros: None,
            current_revision_id: None,
        };
        (tmp, row, first)
    }

    /// The bug: a detached working copy reports `HEAD@<sha>` as its branch, the
    /// IDE sends that back as `?branch=`, and every request 400'd. The label
    /// names the root working copy as it is — exactly what no `branch` names.
    #[tokio::test]
    async fn the_detached_label_resolves_to_the_workspace_root() {
        let (_tmp, row, _first) = workspace_repo();
        let root = PathBuf::from(row.path.clone().unwrap());
        git(&root, &["checkout", "-q", "--detach"]);
        let label = default_git_client()
            .get_current_branch(&root)
            .await
            .unwrap();
        assert!(
            label.starts_with("HEAD@"),
            "precondition: detached, got {label}"
        );

        // A directory that a by-name worktree lookup of the label would find.
        // The label is not a branch, so it must never be looked up as one.
        fs::create_dir_all(root.join(".worktrees").join(&label)).unwrap();

        let resolved = effective_workspace_path(&row, Some(&label))
            .await
            .expect("the label the server emitted must resolve");
        assert_eq!(resolved, root);
        assert_eq!(
            resolved,
            effective_workspace_path(&row, None).await.unwrap(),
            "the detached label and no branch are the same working copy"
        );
    }

    /// A caller cannot use the label's shape to reach anything else: a path, a
    /// non-hex tail, and a real commit that is not HEAD are all refused.
    #[tokio::test]
    async fn forged_detached_labels_are_refused() {
        let (_tmp, row, first) = workspace_repo();
        let root = PathBuf::from(row.path.clone().unwrap());
        git(&root, &["checkout", "-q", "--detach"]);

        let not_head = format!("HEAD@{first}");
        for forged in ["HEAD@../../etc", "HEAD@zzzzzzz", "HEAD@", not_head.as_str()] {
            assert!(
                effective_workspace_path(&row, Some(forged)).await.is_err(),
                "{forged:?} must be refused"
            );
        }
    }

    /// Back on a branch, the old label is refused — even for the commit that
    /// branch points at — and ordinary branch names resolve as they always did.
    #[tokio::test]
    async fn the_label_is_refused_once_the_root_is_on_a_branch() {
        let (_tmp, row, _first) = workspace_repo();
        let root = PathBuf::from(row.path.clone().unwrap());
        let head = git(&root, &["rev-parse", "--short", "HEAD"]);

        assert!(
            effective_workspace_path(&row, Some(&format!("HEAD@{head}")))
                .await
                .is_err()
        );
        assert_eq!(
            effective_workspace_path(&row, Some("main")).await.unwrap(),
            root
        );
        assert!(
            effective_workspace_path(&row, Some("has@symbol"))
                .await
                .is_err()
        );
    }
}
