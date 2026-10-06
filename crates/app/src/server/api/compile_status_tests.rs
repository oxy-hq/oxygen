//! What `/compile/status` says about the serving revision's place in git.
//!
//! The case these exist for was found on a real workspace: it served a
//! snapshot of its disk (`local-…`) compiled months earlier, its files had
//! since been renamed, and the status reported the revision as 0 ahead and 0
//! behind origin — because git cannot compare a string that is not a commit,
//! and the failure was reported as "level".
//!
//! Each test builds a repository on disk: `main` has two commits, and
//! `origin/main` — written as a ref, no network — points at the first.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

use super::{compiled_matches_head, read_git_facts};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        // The developer's own config (signing, hooks) is not under test.
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .args(["-c", "user.name=test", "-c", "user.email=test@example.com"])
        .args(args)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct Repo {
    dir: TempDir,
    /// What `origin/main` points at.
    origin: String,
    /// The working copy's `main`, one commit past origin.
    head: String,
}

fn repo() -> Repo {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path();
    git(path, &["init", "-q", "-b", "main"]);
    git(path, &["commit", "-q", "--allow-empty", "-m", "first"]);
    let origin = git(path, &["rev-parse", "HEAD"]);
    git(path, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(path, &["commit", "-q", "--allow-empty", "-m", "second"]);
    let head = git(path, &["rev-parse", "HEAD"]);
    Repo { dir, origin, head }
}

#[tokio::test]
async fn a_disk_snapshot_is_not_reported_as_level_with_origin() {
    let repo = repo();

    let facts = read_git_facts(repo.dir.path(), "main", Some("local-ce83e24f")).await;

    assert_eq!(facts.head_sha.as_deref(), Some(repo.head.as_str()));
    assert_eq!(facts.remote_sha.as_deref(), Some(repo.origin.as_str()));
    assert_eq!(
        (facts.compiled_ahead, facts.compiled_behind),
        (None, None),
        "a revision that is not a commit cannot be 0 ahead and 0 behind anything"
    );
    assert_eq!(facts.compiled_matches_head, Some(false));
}

/// A real-looking SHA this clone does not have — what a revision compiled
/// from a commit fetched elsewhere looks like from here.
#[tokio::test]
async fn a_commit_this_clone_never_fetched_is_not_reported_as_level_either() {
    let repo = repo();
    let elsewhere = "89abcdef0123456789abcdef0123456789abcdef";

    let facts = read_git_facts(repo.dir.path(), "main", Some(elsewhere)).await;

    assert_eq!((facts.compiled_ahead, facts.compiled_behind), (None, None));
    assert_eq!(facts.compiled_matches_head, Some(false));
}

#[tokio::test]
async fn a_revision_compiled_from_head_matches_it_and_keeps_its_real_position() {
    let repo = repo();

    let facts = read_git_facts(repo.dir.path(), "main", Some(&repo.head)).await;

    assert_eq!(facts.compiled_matches_head, Some(true));
    assert_eq!(
        (facts.compiled_ahead, facts.compiled_behind),
        (Some(1), Some(0)),
        "head is one local commit past origin"
    );
}

/// Level with origin and still not the working copy: the IDE's files have
/// moved on since this was compiled.
#[tokio::test]
async fn a_revision_the_working_copy_has_moved_past_does_not_match_head() {
    let repo = repo();

    let facts = read_git_facts(repo.dir.path(), "main", Some(&repo.origin)).await;

    assert_eq!(
        (facts.compiled_ahead, facts.compiled_behind),
        (Some(0), Some(0))
    );
    assert_eq!(facts.compiled_matches_head, Some(false));
}

#[tokio::test]
async fn with_nothing_promoted_there_is_nothing_to_compare() {
    let repo = repo();

    let facts = read_git_facts(repo.dir.path(), "main", None).await;

    assert_eq!((facts.compiled_ahead, facts.compiled_behind), (None, None));
    assert_eq!(facts.compiled_matches_head, None);
}

#[test]
fn matching_head_needs_both_a_revision_and_a_head() {
    assert_eq!(compiled_matches_head(Some("a"), Some("a")), Some(true));
    assert_eq!(
        compiled_matches_head(Some("local-1"), Some("a")),
        Some(false)
    );
    assert_eq!(compiled_matches_head(None, Some("a")), None);
    assert_eq!(compiled_matches_head(Some("a"), None), None);
}
