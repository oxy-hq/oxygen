use std::path::Path;

use oxy_shared::errors::OxyError;

use crate::cli::{head, repo, run, worktree};
use crate::types::{BranchInfo, BranchOrigin, LocalRefOrigin};

/// Validates that `branch` is a safe branch name for **creating a worktree**
/// (switch, fetch, checkout). Returns precise per-rule errors so the UI can
/// tell the user exactly what to rename.
///
/// Allowed characters: alphanumeric, `.`, `_`, `-`, `/`.
/// Disallowed: `..` sequences, leading `-` or `/`, control chars,
/// **and `--`** (reserved for the worktree directory encoding, see
/// `worktree::branch_to_dir_name`).
pub fn validate_branch_name(branch: &str) -> Result<(), OxyError> {
    validate_branch_name_basic(branch)?;
    if branch.contains("--") {
        let suggested = collapse_dashes(branch);
        return Err(OxyError::RuntimeError(format!(
            "Branch name '{branch}' cannot contain '--' (reserved for the worktree directory \
             encoding). Rename the branch — for example:\n\n  \
             git branch -m '{branch}' '{suggested}'"
        )));
    }
    Ok(())
}

/// Validates that `branch` is a safe name for **read-only / cleanup**
/// operations (delete). Allows `--` so users can clean up legacy branches
/// that pre-date the strict check; still enforces shell-safety.
pub fn validate_branch_name_lenient(branch: &str) -> Result<(), OxyError> {
    validate_branch_name_basic(branch)
}

fn validate_branch_name_basic(branch: &str) -> Result<(), OxyError> {
    if branch.is_empty() {
        return Err(OxyError::RuntimeError(
            "Branch name cannot be empty".to_string(),
        ));
    }
    if branch.starts_with('-') {
        return Err(OxyError::RuntimeError(format!(
            "Branch name '{branch}' cannot start with '-'."
        )));
    }
    if branch.starts_with('/') {
        return Err(OxyError::RuntimeError(format!(
            "Branch name '{branch}' cannot start with '/'."
        )));
    }
    if branch.contains("..") {
        return Err(OxyError::RuntimeError(format!(
            "Branch name '{branch}' cannot contain '..'."
        )));
    }
    let bad_chars: Vec<char> = branch
        .chars()
        .filter(|c| !c.is_alphanumeric() && !matches!(c, '.' | '_' | '-' | '/'))
        .collect();
    if !bad_chars.is_empty() {
        let display: String = bad_chars
            .iter()
            .map(|c| format!("'{c}'"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(OxyError::RuntimeError(format!(
            "Branch name '{branch}' contains invalid character(s): {display}. \
             Allowed: letters, digits, '.', '_', '-', '/'."
        )));
    }
    Ok(())
}

/// Collapses any run of '-' into a single '-' so the suggested rename always
/// satisfies `validate_branch_name`. e.g. `feat--alice` → `feat-alice`,
/// `a---b----c` → `a-b-c`.
fn collapse_dashes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for c in s.chars() {
        if c == '-' {
            if !prev_dash {
                out.push(c);
            }
            prev_dash = true;
        } else {
            out.push(c);
            prev_dash = false;
        }
    }
    out
}

/// Returns the name of the currently checked-out branch in `workspace_root`,
/// or the `HEAD@<short sha>` label when HEAD is detached. The label is owned by
/// [`head`]: it is produced by [`head::HeadState::label`] and recognised by
/// [`head::detached_label_sha`], and it is never a branch name.
pub async fn get_current_branch(workspace_root: &Path) -> Result<String, OxyError> {
    Ok(head::head_state(workspace_root).await?.label())
}

/// Returns `true` if `branch` exists as a local branch in `workspace_root`.
pub(crate) async fn branch_exists(workspace_root: &Path, branch: &str) -> Result<bool, OxyError> {
    let out = run::run(
        workspace_root,
        &["branch", "--list", branch, "--format=%(refname:short)"],
    )
    .await?;
    Ok(!out.trim().is_empty())
}

/// Ensures `branch` exists locally **without touching the current worktree's
/// HEAD or working files**, and reports whether it also exists on origin.
///
/// - If the branch already exists locally, only the origin probe runs.
/// - If the remote has it, runs `git fetch --prune origin` then
///   `git branch --track <branch> origin/<branch>` (creates a local tracking
///   ref; no checkout) and returns `Both`.
/// - Otherwise creates the branch from current `HEAD` via `git branch <branch>`
///   (no checkout) and returns `LocalOnly`.
pub async fn ensure_local_ref(
    workspace_root: &Path,
    branch: &str,
    token: Option<&str>,
) -> Result<LocalRefOrigin, OxyError> {
    validate_branch_name(branch)?;
    let has_remote = repo::has_remote(workspace_root).await;
    let local_exists = branch_exists(workspace_root, branch).await?;

    if local_exists {
        if has_remote && remote_branch_exists(workspace_root, branch).await {
            return Ok(LocalRefOrigin::Both);
        }
        return Ok(LocalRefOrigin::LocalOnly);
    }

    if has_remote {
        let _ = run::run_with_token(workspace_root, &["fetch", "--prune", "origin"], token).await;
        if remote_branch_exists(workspace_root, branch).await {
            let remote_ref = format!("origin/{branch}");
            run::run(workspace_root, &["branch", "--track", branch, &remote_ref]).await?;
            return Ok(LocalRefOrigin::Both);
        }
    }

    run::run(workspace_root, &["branch", branch]).await?;
    Ok(LocalRefOrigin::LocalOnly)
}

async fn remote_branch_exists(workspace_root: &Path, branch: &str) -> bool {
    let remote_ref = format!("origin/{branch}");
    run::run(workspace_root, &["rev-parse", "--verify", &remote_ref])
        .await
        .is_ok()
}

/// Fast-forwards `branch` in `root` from the remote by running
/// `git fetch origin {branch}:{branch}`.
pub async fn fetch_branch_ref(
    root: &Path,
    branch: &str,
    token: Option<&str>,
) -> Result<(), OxyError> {
    validate_branch_name(branch)?;
    let refspec = format!("{branch}:{branch}");
    run::run_with_token(root, &["fetch", "origin", &refspec], token).await?;
    Ok(())
}

/// Lists local branches with their sync status relative to their upstream.
///
/// Returns `(branch_name, sync_status)` where `sync_status` is
/// `"behind"` or `"synced"`.
pub async fn list_branches_with_status(workspace_root: &Path) -> Vec<(String, String)> {
    let default_branch = repo::get_default_branch(workspace_root).await;

    let output = run::run(
        workspace_root,
        &[
            "for-each-ref",
            "--format=%(refname:short)|%(upstream:trackshort)",
            "refs/heads/",
        ],
    )
    .await
    .unwrap_or_default();

    output
        .trim()
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(2, '|');
            let name = parts.next()?.trim().to_string();
            if name.is_empty() {
                return None;
            }
            if name != default_branch {
                let has_worktree = worktree::get_worktree_path(workspace_root, &name)
                    .map(|p| p.exists())
                    .unwrap_or(false);
                if !has_worktree {
                    return None;
                }
            }
            let track = parts.next().unwrap_or("").trim();
            let status = if track == "<" || track == "<>" {
                "behind"
            } else {
                "synced"
            };
            Some((name, status.to_string()))
        })
        .collect()
}

/// Lists every branch the user can switch to — both local and remote-only —
/// with origin and (when applicable) sync status.
///
/// - `Both` branches carry `sync_status = Some("behind"|"synced")`.
/// - `LocalOnly` branches have `sync_status = None` (no upstream to compare).
/// - `RemoteOnly` branches have `sync_status = None` (no local ref).
///
/// Performs a best-effort `git fetch --prune origin` first; fetch failure
/// (e.g. no auth) is swallowed — local + cached remote refs are still
/// returned.
pub async fn list_branches_with_origin(
    workspace_root: &Path,
    token: Option<&str>,
) -> Vec<BranchInfo> {
    if repo::has_remote(workspace_root).await {
        let _ = run::run_with_token(workspace_root, &["fetch", "--prune", "origin"], token).await;
    }

    // Local branches with their tracking-ref status.
    let local_out = run::run(
        workspace_root,
        &[
            "for-each-ref",
            "--format=%(refname:short)|%(upstream:trackshort)",
            "refs/heads/",
        ],
    )
    .await
    .unwrap_or_default();

    let mut by_name: std::collections::BTreeMap<String, BranchInfo> =
        std::collections::BTreeMap::new();

    for line in local_out.trim().lines() {
        let mut parts = line.splitn(2, '|');
        let name = parts.next().unwrap_or("").trim().to_string();
        if name.is_empty() {
            continue;
        }
        let track = parts.next().unwrap_or("").trim();
        // `%(upstream:trackshort)` is `=` synced, `<` behind, `>` ahead,
        // `<>` diverged, or empty when no upstream is configured.
        let sync_status = if track.is_empty() {
            None
        } else if track == "<" || track == "<>" {
            Some("behind".to_string())
        } else {
            Some("synced".to_string())
        };
        by_name.insert(
            name.clone(),
            BranchInfo {
                name,
                origin: BranchOrigin::LocalOnly,
                sync_status,
            },
        );
    }

    // Merge remote branches.
    let remote_out = run::run(
        workspace_root,
        &["branch", "-r", "--format=%(refname:short)"],
    )
    .await
    .unwrap_or_default();

    for line in remote_out.lines() {
        let b = line.trim();
        if b.is_empty() {
            continue;
        }
        let Some(name) = b.strip_prefix("origin/") else {
            continue;
        };
        if name == "HEAD" {
            continue;
        }
        match by_name.get_mut(name) {
            Some(info) => {
                info.origin = BranchOrigin::Both;
                // No upstream configured locally but origin has it: assume
                // "synced" so the badge isn't stuck on "no upstream" until
                // the next push/pull sets tracking.
                if info.sync_status.is_none() {
                    info.sync_status = Some("synced".to_string());
                }
            }
            None => {
                by_name.insert(
                    name.to_string(),
                    BranchInfo {
                        name: name.to_string(),
                        origin: BranchOrigin::RemoteOnly,
                        sync_status: None,
                    },
                );
            }
        }
    }

    by_name.into_values().collect()
}

/// Fetches from origin (best-effort) and returns all branch names the user
/// can check out — both local branches and remote-only branches (stripped
/// of the `origin/` prefix, deduplicated).
pub async fn list_all_branches(
    workspace_root: &Path,
    token: Option<&str>,
) -> Result<Vec<String>, OxyError> {
    if repo::has_remote(workspace_root).await {
        let _ = run::run_with_token(workspace_root, &["fetch", "--prune", "origin"], token).await;
    }

    let local_out = run::run(workspace_root, &["branch", "--format=%(refname:short)"])
        .await
        .unwrap_or_default();
    let mut branches: Vec<String> = local_out
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();

    let remote_out = run::run(
        workspace_root,
        &["branch", "-r", "--format=%(refname:short)"],
    )
    .await
    .unwrap_or_default();
    for line in remote_out.lines() {
        let b = line.trim();
        if b.is_empty() {
            continue;
        }
        let name = b.strip_prefix("origin/").unwrap_or(b);
        if name == "HEAD" {
            continue;
        }
        if !branches.iter().any(|local| local == name) {
            branches.push(name.to_string());
        }
    }

    branches.sort();
    Ok(branches)
}

/// Checks out `branch`.  If the branch only exists on the remote, creates
/// a local tracking branch first.
pub async fn checkout_branch(
    workspace_root: &Path,
    branch: &str,
    token: Option<&str>,
) -> Result<(), OxyError> {
    validate_branch_name(branch)?;
    if repo::has_remote(workspace_root).await {
        let _ = run::run_with_token(workspace_root, &["fetch", "--prune", "origin"], token).await;
    }

    let local_exists = branch_exists(workspace_root, branch).await?;
    if local_exists {
        run::run(workspace_root, &["checkout", branch]).await?;
    } else {
        let remote_ref = format!("origin/{}", branch);
        let remote_exists = run::run(workspace_root, &["rev-parse", "--verify", &remote_ref])
            .await
            .is_ok();

        if remote_exists {
            run::run(workspace_root, &["checkout", "-b", branch, &remote_ref]).await?;
        } else {
            run::run(workspace_root, &["checkout", "-b", branch]).await?;
        }
    }
    Ok(())
}

/// Removes the worktree for `branch` (if any) and deletes the local branch ref.
///
/// Uses the **lenient** validator so users can clean up legacy `--` branches
/// that pre-date the strict `validate_branch_name` rule.
pub async fn delete_branch(workspace_root: &Path, branch: &str) -> Result<(), OxyError> {
    validate_branch_name_lenient(branch)?;
    // Drop stale `.git/worktrees/<dir>` entries from out-of-band removals;
    // without this, `git branch -D` fails with "used by worktree at …" or a
    // future `worktree add` collides on the dirty metadata.
    let _ = run::run(workspace_root, &["worktree", "prune"]).await;
    if let Some(wt_path) = worktree::get_worktree_path(workspace_root, branch) {
        run::run(
            workspace_root,
            &["worktree", "remove", "--force", &wt_path.to_string_lossy()],
        )
        .await?;
    }
    run::run(workspace_root, &["branch", "-D", branch]).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_strict_accepts_normal_names() {
        assert!(validate_branch_name("main").is_ok());
        assert!(validate_branch_name("feature/foo").is_ok());
        assert!(validate_branch_name("user-alice/2026-05-12-1430").is_ok());
        assert!(validate_branch_name("v1.2.3").is_ok());
    }

    #[test]
    fn validate_strict_rejects_double_dash_with_suggested_rename() {
        let err = validate_branch_name("feat--alice").unwrap_err().to_string();
        assert!(err.contains("'--'"), "{err}");
        assert!(
            err.contains("feat-alice"),
            "expected suggested rename in: {err}"
        );
        assert!(
            err.contains("git branch -m 'feat--alice' 'feat-alice'"),
            "expected concrete CLI command in: {err}"
        );
    }

    #[test]
    fn validate_strict_collapses_long_dash_runs_in_suggestion() {
        let err = validate_branch_name("a---b----c").unwrap_err().to_string();
        assert!(
            err.contains("'a-b-c'"),
            "expected collapsed suggestion in: {err}"
        );
    }

    #[test]
    fn validate_strict_pinpoints_other_violations() {
        let cases = [
            ("", "cannot be empty"),
            ("-leading", "cannot start with '-'"),
            ("/leading", "cannot start with '/'"),
            ("has..dotdot", "cannot contain '..'"),
            ("has spaces", "invalid character"),
            ("has@symbol", "invalid character"),
        ];
        for (input, expected_fragment) in cases {
            let err = validate_branch_name(input).unwrap_err().to_string();
            assert!(
                err.contains(expected_fragment),
                "input '{input}' expected to mention '{expected_fragment}', got: {err}"
            );
        }
    }

    #[test]
    fn validate_lenient_allows_double_dash() {
        assert!(validate_branch_name_lenient("feat--alice").is_ok());
        assert!(validate_branch_name_lenient("a---b").is_ok());
    }

    #[test]
    fn validate_lenient_still_rejects_unsafe_inputs() {
        assert!(validate_branch_name_lenient("").is_err());
        assert!(validate_branch_name_lenient("-leading").is_err());
        assert!(validate_branch_name_lenient("has spaces").is_err());
        assert!(validate_branch_name_lenient("has..dotdot").is_err());
    }

    #[test]
    fn distinct_valid_names_map_to_distinct_worktree_dirs() {
        // After strict validation, no two valid branch names should collide
        // under the `/` → `--` encoding.
        let valid = ["feature/foo", "feature/bar", "user/alice", "main", "v1.2.3"];
        let dirs: Vec<String> = valid
            .iter()
            .filter(|b| validate_branch_name(b).is_ok())
            .map(|b| crate::cli::worktree::branch_to_dir_name(b))
            .collect();
        let mut sorted = dirs.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), dirs.len(), "collision in {dirs:?}");
    }

    /// Returns `(workdir, local_path)`. `workdir` owns the bare remote and
    /// must be kept alive for the duration of the test — dropping it
    /// removes the remote and the local clone's origin URL becomes a stale
    /// path. `local_path` is the clone to run `list_branches_with_origin`
    /// against, with three branch states:
    /// - `main`: Both (local + origin)
    /// - `local-only`: LocalOnly (never pushed)
    /// - `remote-only`: RemoteOnly (pushed from a sibling clone)
    async fn setup_three_origin_states() -> (tempfile::TempDir, std::path::PathBuf) {
        use tokio::process::Command;
        async fn g(dir: &Path, args: &[&str]) -> String {
            let out = Command::new("git")
                .current_dir(dir)
                .args(args)
                .output()
                .await
                .expect("spawn git");
            String::from_utf8_lossy(&out.stdout).into_owned()
        }

        let workdir = tempfile::TempDir::new().unwrap();
        let bare = workdir.path().join("remote.git");
        let local = workdir.path().join("local");
        let other = workdir.path().join("other");

        g(
            workdir.path(),
            &["init", "--bare", "-b", "main", bare.to_str().unwrap()],
        )
        .await;
        g(
            workdir.path(),
            &["clone", bare.to_str().unwrap(), local.to_str().unwrap()],
        )
        .await;

        g(&local, &["config", "user.email", "a@a"]).await;
        g(&local, &["config", "user.name", "A"]).await;
        tokio::fs::write(local.join("shared.txt"), "v1\n")
            .await
            .unwrap();
        g(&local, &["add", "."]).await;
        g(&local, &["commit", "-m", "initial"]).await;
        g(&local, &["push", "-u", "origin", "main"]).await;

        // local-only: never pushed
        g(&local, &["checkout", "-b", "local-only"]).await;
        tokio::fs::write(local.join("local.txt"), "x")
            .await
            .unwrap();
        g(&local, &["add", "."]).await;
        g(&local, &["commit", "-m", "local"]).await;
        g(&local, &["checkout", "main"]).await;

        // remote-only: pushed from another clone, never fetched as a local branch here
        g(
            workdir.path(),
            &["clone", bare.to_str().unwrap(), other.to_str().unwrap()],
        )
        .await;
        g(&other, &["config", "user.email", "b@b"]).await;
        g(&other, &["config", "user.name", "B"]).await;
        g(&other, &["checkout", "-b", "remote-only"]).await;
        tokio::fs::write(other.join("r.txt"), "y").await.unwrap();
        g(&other, &["add", "."]).await;
        g(&other, &["commit", "-m", "remote"]).await;
        g(&other, &["push", "-u", "origin", "remote-only"]).await;

        (workdir, local)
    }

    #[tokio::test]
    async fn list_branches_with_origin_reports_all_three_states() {
        // Keep `_workdir` alive — it owns the bare remote that origin/<branch>
        // resolves against. Dropping it before the test runs deletes the
        // remote out from under us.
        let (_workdir, local) = setup_three_origin_states().await;
        let infos = list_branches_with_origin(&local, None).await;

        let by_name: std::collections::HashMap<&str, &BranchInfo> =
            infos.iter().map(|i| (i.name.as_str(), i)).collect();

        let main = by_name.get("main").expect("main should be listed");
        assert_eq!(main.origin, BranchOrigin::Both, "main should be Both");

        let local_only = by_name
            .get("local-only")
            .expect("local-only should be listed");
        assert_eq!(local_only.origin, BranchOrigin::LocalOnly);
        assert_eq!(local_only.sync_status, None);

        let remote_only = by_name
            .get("remote-only")
            .expect("remote-only should be listed");
        assert_eq!(remote_only.origin, BranchOrigin::RemoteOnly);
        assert_eq!(remote_only.sync_status, None);
    }
}
