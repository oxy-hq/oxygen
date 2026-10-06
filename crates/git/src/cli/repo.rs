use std::collections::HashMap;
use std::path::{Path, PathBuf};

use oxy_shared::errors::OxyError;
use tracing::info;

use crate::cli::{config, run};

/// Per-workspace cache for the default branch name, keyed on workspace root.
/// Computed on first call per workspace.  Multi-tenant deployments host
/// workspaces with different default branches, so a single process-global
/// value is insufficient.
static DEFAULT_BRANCH: std::sync::OnceLock<std::sync::Mutex<HashMap<PathBuf, String>>> =
    std::sync::OnceLock::new();

/// Walks up the directory tree from `path` and returns the first ancestor
/// (inclusive) that contains a `.git` entry, or `None` if none is found.
///
/// This mirrors the discovery behaviour of the `git` binary itself: a
/// workspace that lives inside a larger repository (i.e. `.git` is in a
/// parent directory) is still considered part of that repository.
pub fn find_git_root(path: &Path) -> Option<PathBuf> {
    let mut dir = path;
    loop {
        if dir.join(".git").exists() {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
}

/// Returns `true` if `workspace_root` is inside a git repository.
///
/// The `.git` directory may be in any ancestor of `workspace_root`, not
/// necessarily in `workspace_root` itself.
pub fn is_git_repo(workspace_root: &Path) -> bool {
    find_git_root(workspace_root).is_some()
}

/// Where `workspace_root` sits inside the repository that contains it, as a
/// `/`-separated relative path. `None` at the repository root, outside any
/// repository, or when the path is not UTF-8.
///
/// Both paths are canonicalized first so a symlink or a `..` component cannot
/// make `strip_prefix` answer with an empty or wrong remainder.
pub fn subdir_in_repo(workspace_root: &Path) -> Option<String> {
    let git_root = find_git_root(workspace_root)?;
    let canon_workspace = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf());
    let canon_root = git_root.canonicalize().unwrap_or(git_root);
    canon_workspace
        .strip_prefix(&canon_root)
        .ok()
        .and_then(|p| p.to_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.replace('\\', "/"))
}

/// Initialises a git repository at `workspace_root` if one does not already
/// exist, then creates an initial commit so the repo has at least one
/// reachable commit on `main`.
///
/// No-op when `.git` already exists.
pub async fn ensure_initialized(workspace_root: &Path) -> Result<(), OxyError> {
    if is_git_repo(workspace_root) {
        info!(
            "Local git repo already exists at {}, skipping init",
            workspace_root.display()
        );
        return Ok(());
    }

    info!(
        "Initialising local git repo at {}",
        workspace_root.display()
    );

    run::run(workspace_root, &["init", "-b", "main"]).await?;
    config::ensure_user_config().await?;
    run::run(workspace_root, &["add", "-A"]).await?;

    // Empty project — initial commit is allowed to fail.
    let _ = run::run(
        workspace_root,
        &["commit", "-m", "Initial commit: Oxy project"],
    )
    .await;

    info!("Local git repo initialised at {}", workspace_root.display());
    Ok(())
}

/// Returns `true` if `workspace_root` has at least one configured git remote.
pub async fn has_remote(workspace_root: &Path) -> bool {
    run::run(workspace_root, &["remote"])
        .await
        .map(|out| !out.trim().is_empty())
        .unwrap_or(false)
}

/// Resolves the actual git directory for `root`.
///
/// For a regular repo, this is `<git-root>/.git/` where `<git-root>` may be
/// `root` itself or any ancestor (workspaces in a repo subfolder).
/// For a git worktree, `<git-root>/.git` is a file containing `gitdir: <path>` —
/// we follow the pointer so callers find the per-worktree state directory.
/// This covers both "worktree at root" and "subfolder of a worktree".
pub(crate) fn resolve_git_dir(root: &Path) -> PathBuf {
    let Some(git_root) = find_git_root(root) else {
        return root.join(".git");
    };
    let dot_git = git_root.join(".git");
    follow_dot_git(&git_root, dot_git)
}

/// Given a `<dir>/.git` path, returns the actual git object directory.
///
/// For a linked worktree, `.git` is a file containing `gitdir: <rel>` —
/// the pointer is followed.  For a regular checkout, `.git` is a directory
/// and is returned unchanged.
fn follow_dot_git(dir: &Path, dot_git: PathBuf) -> PathBuf {
    if dot_git.is_file()
        && let Ok(content) = std::fs::read_to_string(&dot_git)
        && let Some(rel) = content.trim().strip_prefix("gitdir: ")
    {
        let resolved = dir.join(rel);
        if let Ok(canonical) = resolved.canonicalize() {
            return canonical;
        }
    }
    dot_git
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn find_git_root_directly_at_path() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        assert_eq!(find_git_root(dir.path()), Some(dir.path().to_path_buf()));
    }

    #[test]
    fn find_git_root_in_subfolder() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        let sub = dir.path().join("workspace").join("nested");
        fs::create_dir_all(&sub).unwrap();
        assert_eq!(find_git_root(&sub), Some(dir.path().to_path_buf()));
    }

    #[test]
    fn find_git_root_no_repo() {
        let dir = TempDir::new().unwrap();
        assert_eq!(find_git_root(dir.path()), None);
    }

    #[test]
    fn subdir_in_repo_is_the_slash_separated_path_below_the_git_root() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        let sub = dir.path().join("data").join("oxy");
        fs::create_dir_all(&sub).unwrap();
        assert_eq!(subdir_in_repo(&sub).as_deref(), Some("data/oxy"));
    }

    #[test]
    fn subdir_in_repo_is_none_at_the_root_and_outside_a_repository() {
        let repo = TempDir::new().unwrap();
        fs::create_dir(repo.path().join(".git")).unwrap();
        assert_eq!(subdir_in_repo(repo.path()), None);

        let bare = TempDir::new().unwrap();
        assert_eq!(subdir_in_repo(bare.path()), None);
    }

    #[test]
    fn is_git_repo_in_subfolder() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        let sub = dir.path().join("oxy-workspace");
        fs::create_dir(&sub).unwrap();
        assert!(is_git_repo(&sub));
    }

    #[test]
    fn is_git_repo_false_outside_repo() {
        // Parent of a temp dir that has no .git anywhere in the chain
        // (the temp dir itself has no .git either).
        let dir = TempDir::new().unwrap();
        assert!(!is_git_repo(dir.path()));
    }

    #[test]
    fn resolve_git_dir_follows_worktree_file_in_subfolder() {
        // Layout: repo/.git/ (real dir)
        //         repo/.worktrees/feat/.git → file pointing to real gitdir
        //         repo/.worktrees/feat/sub/workspace (subfolder of linked worktree)
        let dir = TempDir::new().unwrap();
        let real_gitdir = dir.path().join(".git");
        let worktree_gitdir = real_gitdir.join("worktrees").join("feat");
        fs::create_dir_all(&worktree_gitdir).unwrap();

        let worktree_root = dir.path().join(".worktrees").join("feat");
        fs::create_dir_all(&worktree_root).unwrap();
        // .git file uses a relative path back to the real gitdir
        let pointer_content = format!(
            "gitdir: {}",
            worktree_gitdir
                .strip_prefix(&worktree_root)
                .unwrap_or(&worktree_gitdir)
                .display()
        );
        // Use the absolute path for simplicity in the test
        fs::write(
            worktree_root.join(".git"),
            format!("gitdir: {}", worktree_gitdir.display()),
        )
        .unwrap();
        let _ = pointer_content;

        let sub = worktree_root.join("sub").join("workspace");
        fs::create_dir_all(&sub).unwrap();

        // resolve_git_dir on the subfolder must follow the gitdir pointer,
        // not return the .git file path directly.
        let resolved = resolve_git_dir(&sub);
        assert_eq!(resolved, worktree_gitdir);
    }
}

/// The process-wide default-branch override, `GIT_DEFAULT_BRANCH`, when it is
/// set to something. It answers for every workspace before git is asked
/// ([`get_default_branch`]), so anything that answers the same question from
/// another source has to give way to it too, or two nodes with the same
/// environment disagree.
pub fn default_branch_override() -> Option<String> {
    std::env::var("GIT_DEFAULT_BRANCH")
        .ok()
        .filter(|b| !b.is_empty())
}

/// The branch `origin/HEAD` names in the checkout at `workspace_root`, or
/// `None` when git cannot say (no repository, no remote, `origin/HEAD` unset).
///
/// This is the one answer [`get_default_branch`] gives that is a fact about
/// the repository: its other two are a process-wide override and a guess. A
/// caller that records the default branch somewhere durable wants only this.
pub async fn remote_default_branch(workspace_root: &Path) -> Option<String> {
    let out = run::run(
        workspace_root,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .await
    .ok()?;
    let s = out.trim().to_string();
    Some(s.strip_prefix("origin/").map(str::to_string).unwrap_or(s))
}

/// Returns the default branch name for `workspace_root`.
///
/// Resolution order:
/// 1. `GIT_DEFAULT_BRANCH` env var
/// 2. `git symbolic-ref --short refs/remotes/origin/HEAD`
/// 3. `"main"`
pub async fn get_default_branch(workspace_root: &Path) -> String {
    let cache = DEFAULT_BRANCH.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    if let Some(cached) = cache.lock().unwrap().get(workspace_root) {
        return cached.clone();
    }
    // `resolved` distinguishes an authoritative answer (env var or a
    // successful `symbolic-ref`) from the `"main"` fallback we return when the
    // git lookup errors (shallow clone, `origin/HEAD` unset, transient failure).
    // Only authoritative answers are cached: caching the fallback would pin the
    // wrong default for the whole process lifetime and silently defeat any
    // caller-side TTL — a repo whose real default is `master` would be stuck on
    // `main` after a single transient error. Returning (but not caching) the
    // fallback keeps behaviour unchanged for genuinely git-less paths while
    // letting a transient error self-heal on the next call.
    let (resolved, value) = if let Some(b) = default_branch_override() {
        (true, b)
    } else {
        match remote_default_branch(workspace_root).await {
            Some(branch) => (true, branch),
            None => (false, "main".to_string()),
        }
    };
    if resolved {
        cache
            .lock()
            .unwrap()
            .insert(workspace_root.to_path_buf(), value.clone());
    }
    value
}
