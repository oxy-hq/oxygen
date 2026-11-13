use std::path::Path;

use axum::http::StatusCode;
use tempfile::TempDir;

use super::*;

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

/// A repo on `main` with one commit. Returns `(dir, short_sha)`.
fn repo() -> (TempDir, String) {
    let tmp = TempDir::new().expect("tempdir");
    let dir = tmp.path();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "t@example.com"]);
    git(dir, &["config", "user.name", "Test"]);
    std::fs::write(dir.join("config.yml"), "one").expect("seed file");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "first"]);
    let sha = git(dir, &["rev-parse", "--short", "HEAD"]);
    (tmp, sha)
}

fn detached_repo() -> (TempDir, String) {
    let (tmp, sha) = repo();
    git(tmp.path(), &["checkout", "-q", "--detach"]);
    (tmp, sha)
}

/// Workspace details report the label as `active_branch` and the sha beside
/// it, so the frontend can say "Detached at <sha>" without parsing a label.
#[tokio::test]
async fn details_report_the_label_and_its_sha() {
    let (tmp, sha) = detached_repo();
    assert_eq!(
        active_head(tmp.path(), "main").await,
        (format!("HEAD@{sha}"), Some(sha))
    );

    let (on_branch, _sha) = repo();
    assert_eq!(
        active_head(on_branch.path(), "main").await,
        ("main".to_string(), None)
    );
}

/// A stale label is a conflict with the workspace's state; a value that is not
/// a label at all is still a bad request.
#[test]
fn only_a_well_formed_label_is_a_conflict() {
    assert_eq!(
        unresolved_branch_status(Some("HEAD@abc1234")),
        StatusCode::CONFLICT
    );
    for value in [Some("HEAD@../../etc"), Some("has@symbol"), Some(""), None] {
        assert_eq!(
            unresolved_branch_status(value),
            StatusCode::BAD_REQUEST,
            "{value:?}"
        );
    }
}
