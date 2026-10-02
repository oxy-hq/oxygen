use std::path::Path;

use axum::http::StatusCode;
use axum::response::Response;
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

async fn body_of(response: Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("json body")
}

/// The refusal a branch-only operation gives: `409`, the code the frontend
/// keys on, the sha, and a message that says what to do.
async fn assert_detached_refusal(refusal: DetachedHead, sha: &str) {
    // What a handler's `?` turns the refusal into.
    let response: Response = GitRefusal::from(refusal).into_response();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = body_of(response).await;
    assert_eq!(body["code"], DETACHED_HEAD_CODE);
    assert_eq!(body["sha"], sha);
    assert_eq!(
        body["message"],
        format!(
            "This workspace is on a detached HEAD at {sha}; switch to or create a branch first."
        )
    );
}

/// Push, force-push and restore guard on the working copy itself.
#[tokio::test]
async fn a_branch_only_operation_on_a_detached_head_answers_409() {
    let (tmp, sha) = detached_repo();
    let refusal = require_attached_head(tmp.path())
        .await
        .expect_err("a detached HEAD has no branch to act on");
    assert_detached_refusal(refusal, &sha).await;
}

/// Pull and fetch take a branch operand. The label is not one, and neither is
/// any other name when the working copy it resolved to is detached — `main`
/// resolves to the root too, and `git pull --rebase origin main` there would
/// rebase a HEAD that no branch holds.
#[tokio::test]
async fn a_branch_operand_is_refused_while_detached_whatever_it_is_called() {
    let (tmp, sha) = detached_repo();
    let label = format!("HEAD@{sha}");
    for operand in [Some(label), Some("main".to_string()), None] {
        let refusal = require_branch(operand.clone(), tmp.path())
            .await
            .expect_err("no branch while detached");
        assert_detached_refusal(refusal, &sha).await;
    }
}

/// Control: on a branch nothing is refused, and the operand is what it was.
#[tokio::test]
async fn on_a_branch_the_operation_proceeds() {
    let (tmp, _sha) = repo();
    assert!(require_attached_head(tmp.path()).await.is_ok());
    assert_eq!(
        require_branch(Some("main".to_string()), tmp.path())
            .await
            .expect("on main"),
        "main"
    );
    assert_eq!(
        require_branch(None, tmp.path()).await.expect("default"),
        "main"
    );
}

/// A rebase detaches HEAD while it runs. That state keeps its own handling
/// (the push refusal that names the rebase, Resolve / abort / continue), so the
/// detached guard stands aside.
#[tokio::test]
async fn a_rebase_in_progress_is_left_to_its_own_handling() {
    let (tmp, _sha) = detached_repo();
    std::fs::create_dir(tmp.path().join(".git").join("rebase-merge")).expect("rebase marker");
    assert!(require_attached_head(tmp.path()).await.is_ok());
}

/// Revision info for the label describes the commit HEAD names, with no
/// upstream — not an empty answer from looking the label up as a branch ref.
#[tokio::test]
async fn the_revision_tip_of_a_detached_head_is_head_with_no_upstream() {
    let (tmp, sha) = detached_repo();
    let tip = revision_tip(tmp.path(), &format!("HEAD@{sha}")).await;
    assert!(tip.detached);
    assert!(tip.sha.starts_with(&sha), "{} vs {sha}", tip.sha);
    assert_eq!(tip.message, "first");
    assert_eq!(tip.tracking_sha, None);

    let on_main = revision_tip(tmp.path(), "main").await;
    assert!(!on_main.detached);
    assert_eq!(on_main.sha, tip.sha);
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
