use std::path::{Path, PathBuf};

use oxy_shared::errors::OxyError;
use tracing::info;

use crate::cli::{branch, config, repo, run};

/// Directory name for git worktrees inside the project root.
pub const WORKTREES_DIR: &str = ".worktrees";

/// Converts a branch name to a safe directory name by encoding `/` as `--`.
/// Bijective only because `branch::validate_branch_name` rejects names
/// containing `--`.
pub(crate) fn branch_to_dir_name(branch: &str) -> String {
    branch.replace('/', "--")
}

/// Returns the worktree path for `branch` if it exists on disk.
pub fn get_worktree_path(workspace_root: &Path, branch: &str) -> Option<PathBuf> {
    if branch.is_empty() {
        return None;
    }
    let dir = branch_to_dir_name(branch);
    let path = workspace_root.join(WORKTREES_DIR).join(&dir);
    if path.exists() { Some(path) } else { None }
}

/// Ensures the repo ignores its own `.worktrees/` directory via the **local**
/// exclude file, so the worktrees Oxy creates inside the main working copy never
/// surface as untracked changes on the main branch.
///
/// The entry is written to `<git-common-dir>/info/exclude` — a per-clone,
/// never-committed ignore file shared across all of a repo's worktrees — rather
/// than the tracked `.gitignore`, so the customer's repository history is left
/// untouched (nothing to commit, nothing to push). The pattern is unanchored so
/// it also covers subdirectory workspaces, where `.worktrees/` sits below the
/// repo root. Idempotent, and best-effort: a failure here only leaves status
/// noisy, so it warns rather than aborting worktree creation.
async fn ensure_worktrees_ignored(workspace_root: &Path) {
    let common_dir = match run::run(workspace_root, &["rev-parse", "--git-common-dir"]).await {
        Ok(out) if !out.trim().is_empty() => out.trim().to_string(),
        Ok(_) => return,
        Err(e) => {
            tracing::warn!(error = %e, "could not resolve git-common-dir; skipping .worktrees exclude");
            return;
        }
    };
    // `--git-common-dir` is relative to `workspace_root` for a normal checkout
    // and absolute for a linked worktree; `join` does the right thing for both.
    let exclude_path = workspace_root
        .join(&common_dir)
        .join("info")
        .join("exclude");

    // Recognise every spelling git accepts for this ignore so we never
    // double-write: unanchored with/without the trailing slash, and the
    // repo-root-anchored forms a user might have hand-written.
    let entry = format!("{WORKTREES_DIR}/");
    let anchored = format!("/{entry}");
    let anchored_bare = format!("/{WORKTREES_DIR}");
    let existing = tokio::fs::read_to_string(&exclude_path)
        .await
        .unwrap_or_default();
    if existing.lines().any(|l| {
        let t = l.trim();
        t == entry || t == WORKTREES_DIR || t == anchored || t == anchored_bare
    }) {
        return;
    }

    if let Some(parent) = exclude_path.parent()
        && let Err(e) = tokio::fs::create_dir_all(parent).await
    {
        tracing::warn!(error = %e, "could not create git info dir; skipping .worktrees exclude");
        return;
    }

    // This read-modify-write is not atomic: two concurrent first-time calls for
    // the same repo (worktree creation can race — see the exists() recovery
    // below) could both append. Harmless — a duplicate exclude line is a no-op,
    // and the dedup check above makes it self-limiting to at most one extra line.
    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(&entry);
    content.push('\n');
    if let Err(e) = tokio::fs::write(&exclude_path, content).await {
        tracing::warn!(error = %e, "could not write .worktrees exclude entry");
    }
}

/// Returns the worktree path for `branch`, creating the worktree (and the
/// branch, if it does not already exist) when necessary.
///
/// The branch is forked from `HEAD` of the main project directory, so the
/// new branch starts with a clean copy of the current project state.
pub async fn get_or_create_worktree(
    workspace_root: &Path,
    branch_name: &str,
) -> Result<PathBuf, OxyError> {
    let default_branch = repo::get_default_branch(workspace_root).await;
    if branch_name.is_empty() || branch_name == default_branch {
        return Ok(workspace_root.to_path_buf());
    }

    branch::validate_branch_name(branch_name)?;

    // Git allows a branch to be checked out in at most one worktree at a time.
    // If `branch_name` is already checked out somewhere in this repo — most
    // commonly the main working copy itself, when the repo's main checkout
    // sits on a non-default branch — reuse that checkout instead of asking
    // git to add a second one, which it refuses with "already used by
    // worktree at ...".
    if let Some(existing) = find_branch_checkout(workspace_root, branch_name).await? {
        return Ok(existing);
    }

    // The worktree lives at `<workspace_root>/.worktrees/<branch>`, i.e. nested
    // inside the main working copy. Git reports that nested directory as an
    // untracked entry (`?? .worktrees/`) in the main branch's status, which
    // dirties main, blocks clean pulls, and tempts a spurious commit. Ignore it
    // locally before it is ever created. Runs before the `exists()` early-return
    // below so workspaces whose `.worktrees/` predates this fix self-heal on the
    // next access.
    ensure_worktrees_ignored(workspace_root).await;

    let dir_name = branch_to_dir_name(branch_name);
    let worktree_path = workspace_root.join(WORKTREES_DIR).join(&dir_name);

    if worktree_path.exists() {
        return Ok(worktree_path);
    }

    tokio::fs::create_dir_all(workspace_root.join(WORKTREES_DIR))
        .await
        .map_err(|e| OxyError::IOError(format!("Failed to create .worktrees dir: {e}")))?;

    config::ensure_user_config().await?;

    add_worktree(workspace_root, branch_name, &worktree_path).await?;

    info!(
        "Created git worktree '{}' at {}",
        branch_name,
        worktree_path.display()
    );
    Ok(worktree_path)
}

/// Runs `git worktree add` for `branch_name` at `worktree_path`, creating the
/// branch first when it does not already exist locally.
///
/// Tolerates losing a creation race: if the command fails but the directory
/// materialised anyway, a concurrent caller won and that counts as success.
/// Any other failure (including the branch having become checked out
/// elsewhere between [`get_or_create_worktree`]'s upfront check and this
/// call) is returned as-is — the caller decides how to surface it.
async fn add_worktree(
    workspace_root: &Path,
    branch_name: &str,
    worktree_path: &Path,
) -> Result<(), OxyError> {
    let branch_exists = branch::branch_exists(workspace_root, branch_name).await?;

    let result = if branch_exists {
        run::run(
            workspace_root,
            &[
                "worktree",
                "add",
                &worktree_path.to_string_lossy(),
                branch_name,
            ],
        )
        .await
    } else {
        run::run(
            workspace_root,
            &[
                "worktree",
                "add",
                "-b",
                branch_name,
                &worktree_path.to_string_lossy(),
            ],
        )
        .await
    };

    match result {
        Ok(_) => Ok(()),
        Err(_) if worktree_path.exists() => {
            info!(
                "Worktree at {} already exists (concurrent creation), using it",
                worktree_path.display()
            );
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// A worktree as reported by `git worktree list --porcelain`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeEntry {
    /// Absolute path to the worktree's working directory.
    pub path: PathBuf,
    /// Branch ref checked out there (e.g. `refs/heads/feature`), or `None` for
    /// a detached HEAD.
    pub branch: Option<String>,
    /// True for the repo's main working tree (always git's first record). The
    /// reaper never touches it.
    pub is_main: bool,
}

/// Lists every worktree git knows about for the repo containing
/// `workspace_root`, including the main working tree (flagged `is_main`).
pub async fn list_worktrees(workspace_root: &Path) -> Result<Vec<WorktreeEntry>, OxyError> {
    let out = run::run(workspace_root, &["worktree", "list", "--porcelain"]).await?;
    Ok(parse_worktree_list(&out))
}

/// Finds the worktree — if any — where `branch` is currently checked out,
/// including the repo's main working copy. Git allows a branch to be checked
/// out in at most one worktree at a time, so this is authoritative: when it
/// returns `Some`, a `git worktree add` for that branch is doomed to fail
/// with "already used by worktree at ...".
///
/// The single source of truth for "where does this branch actually live on
/// disk" — shared by [`get_or_create_worktree`] (to skip a doomed `add`) and
/// `oxy::adapters::workspace::effective_workspace_path` (so reads of the
/// branch land on the same checkout), so the two can never disagree.
pub async fn find_branch_checkout(
    workspace_root: &Path,
    branch: &str,
) -> Result<Option<PathBuf>, OxyError> {
    let target = format!("refs/heads/{branch}");
    let entries = list_worktrees(workspace_root).await?;
    Ok(entries
        .into_iter()
        .find(|e| e.branch.as_deref() == Some(target.as_str()))
        .map(|e| e.path))
}

/// Parses `git worktree list --porcelain`. Records are blank-line separated;
/// each starts with a `worktree <path>` line, optionally followed by
/// `branch <ref>` (absent for detached/bare). Git always lists the main
/// working tree first, so the first parsed entry is `is_main`.
fn parse_worktree_list(porcelain: &str) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut path: Option<PathBuf> = None;
    let mut branch: Option<String> = None;
    for line in porcelain.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            if let Some(path) = path.take() {
                entries.push(WorktreeEntry {
                    is_main: entries.is_empty(),
                    path,
                    branch: branch.take(),
                });
            }
            path = Some(PathBuf::from(p));
            branch = None;
        } else if let Some(b) = line.strip_prefix("branch ") {
            branch = Some(b.to_string());
        }
    }
    if let Some(path) = path {
        entries.push(WorktreeEntry {
            is_main: entries.is_empty(),
            path,
            branch,
        });
    }
    entries
}

/// Returns true when the worktree at `worktree_path` has no uncommitted changes
/// (`git status --porcelain` is empty). A clean worktree is safe to discard:
/// every change is committed and lives in the repo's shared object store, so
/// removing the working copy loses nothing — the branch ref and its history
/// survive.
pub async fn is_worktree_clean(worktree_path: &Path) -> Result<bool, OxyError> {
    let out = run::run(worktree_path, &["status", "--porcelain"]).await?;
    Ok(out.trim().is_empty())
}

/// Removes the worktree at `worktree_path` **without** deleting its branch ref.
///
/// Unlike [`branch::delete_branch`] (an explicit user action that also runs
/// `branch -D`), this discards only the on-disk working copy — the branch and
/// its commits remain, and the next access re-materialises the worktree via
/// [`get_or_create_worktree`]. Deliberately no `--force`: if the worktree went
/// dirty since the caller's clean-check, `remove` fails closed rather than
/// silently discarding work. `prune` first clears stale metadata from any
/// out-of-band removals.
pub async fn remove_worktree(workspace_root: &Path, worktree_path: &Path) -> Result<(), OxyError> {
    run::run(workspace_root, &["worktree", "prune"]).await?;
    run::run(
        workspace_root,
        &["worktree", "remove", &worktree_path.to_string_lossy()],
    )
    .await?;
    info!("Reaped idle worktree at {}", worktree_path.display());
    Ok(())
}

/// Recursively copies `src` to `dst` using `tokio::fs`.
///
/// Used as a fallback when `rename` fails with EXDEV (cross-device mount).
pub(crate) async fn copy_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        tokio::fs::create_dir_all(dst).await?;
        let mut entries = tokio::fs::read_dir(src).await?;
        while let Some(entry) = entries.next_entry().await? {
            let child_dst = dst.join(entry.file_name());
            Box::pin(copy_recursive(&entry.path(), &child_dst)).await?;
        }
    } else {
        tokio::fs::copy(src, dst).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_main_plus_worktrees_and_detached() {
        let porcelain = "\
worktree /state/ws/abc
HEAD 1111111111111111111111111111111111111111
branch refs/heads/main

worktree /state/ws/abc/.worktrees/feature
HEAD 2222222222222222222222222222222222222222
branch refs/heads/feature

worktree /state/ws/abc/.worktrees/wip
HEAD 3333333333333333333333333333333333333333
detached
";
        let entries = parse_worktree_list(porcelain);
        assert_eq!(entries.len(), 3);
        // First record is always the main working tree.
        assert!(entries[0].is_main);
        assert_eq!(entries[0].branch.as_deref(), Some("refs/heads/main"));
        // The .worktrees/* entries are never main.
        assert!(!entries[1].is_main);
        assert_eq!(
            entries[1].path,
            PathBuf::from("/state/ws/abc/.worktrees/feature")
        );
        assert_eq!(entries[1].branch.as_deref(), Some("refs/heads/feature"));
        // Detached HEAD → no branch ref.
        assert!(!entries[2].is_main);
        assert_eq!(entries[2].branch, None);
    }

    #[test]
    fn single_worktree_is_main() {
        let entries =
            parse_worktree_list("worktree /state/ws/abc\nHEAD abc\nbranch refs/heads/main\n");
        assert_eq!(entries.len(), 1);
        assert!(entries[0].is_main);
    }

    #[test]
    fn empty_output_yields_no_entries() {
        assert!(parse_worktree_list("").is_empty());
    }

    async fn git(cwd: &Path, args: &[&str]) -> String {
        let out = tokio::process::Command::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .await
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Regression: creating a worktree under `<root>/.worktrees/` must NOT leave
    /// the main working copy dirty. Before the fix, `git status` reported
    /// `?? .worktrees/`, which forced a spurious commit before main could pull.
    #[tokio::test]
    async fn worktree_creation_keeps_main_clean_via_local_exclude() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path();
        git(repo, &["init", "-q", "-b", "main"]).await;
        git(repo, &["config", "user.email", "t@example.com"]).await;
        git(repo, &["config", "user.name", "Test"]).await;
        std::fs::write(repo.join("f.txt"), "hi").expect("seed file");
        git(repo, &["add", "."]).await;
        git(repo, &["commit", "-qm", "init"]).await;

        assert!(
            git(repo, &["status", "--porcelain"])
                .await
                .trim()
                .is_empty(),
            "precondition: repo is clean before any worktree"
        );

        let wt = get_or_create_worktree(repo, "feature")
            .await
            .expect("worktree created");
        assert!(wt.exists(), "worktree directory materialised");
        assert!(wt.starts_with(repo.join(WORKTREES_DIR)));

        let status = git(repo, &["status", "--porcelain"]).await;
        assert!(
            status.trim().is_empty(),
            "main must stay clean after worktree creation, got: {status:?}"
        );

        // Idempotent: a second call must not duplicate the exclude entry.
        get_or_create_worktree(repo, "feature")
            .await
            .expect("idempotent re-access");
        let exclude = std::fs::read_to_string(repo.join(".git").join("info").join("exclude"))
            .unwrap_or_default();
        let count = exclude
            .lines()
            .filter(|l| l.trim() == ".worktrees/")
            .count();
        assert_eq!(count, 1, "exclude entry written exactly once:\n{exclude}");
    }

    /// Regression: when the repo's main working copy is itself checked out on
    /// the branch being staged — a non-default branch, e.g. an ephemeral
    /// environment whose root sits on its feature branch — `get_or_create_worktree`
    /// must reuse that checkout rather than ask git to `worktree add` a second
    /// one for the same branch. Git refuses that outright:
    /// `fatal: '<branch>' is already used by worktree at '<root>'`, which
    /// before the fix surfaced as exactly this function returning `Err`.
    #[tokio::test]
    async fn get_or_create_worktree_reuses_main_checkout_already_on_the_branch() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path();
        git(repo, &["init", "-q", "-b", "main"]).await;
        git(repo, &["config", "user.email", "t@example.com"]).await;
        git(repo, &["config", "user.name", "Test"]).await;
        std::fs::write(repo.join("f.txt"), "hi").expect("seed file");
        git(repo, &["add", "."]).await;
        git(repo, &["commit", "-qm", "init"]).await;

        // The main working copy moves to a non-default branch — "feature" is
        // not git's default (git's fallback here is "main", and this repo has
        // no remote to say otherwise), so this does not hit the
        // `branch_name == default_branch` early return.
        git(repo, &["checkout", "-q", "-b", "feature"]).await;

        let wt = get_or_create_worktree(repo, "feature")
            .await
            .expect("must reuse the main checkout instead of erroring");

        // `git worktree list` reports a canonicalized path (e.g. resolving
        // macOS's `/var` -> `/private/var` symlink), so compare against the
        // canonical form of `repo` rather than the raw tempdir path.
        let canonical_repo = std::fs::canonicalize(repo).expect("repo path resolves");
        assert_eq!(
            wt, canonical_repo,
            "branch already checked out at the main working copy must resolve there, \
             not a new .worktrees/ entry"
        );
        assert!(
            !repo.join(WORKTREES_DIR).join("feature").exists(),
            "must not have attempted (or succeeded) to add a second worktree for the same branch"
        );
    }
}
