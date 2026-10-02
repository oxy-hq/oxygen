use std::path::Path;

use tempfile::TempDir;

use super::*;
use crate::cli::{branch, push_pull};

async fn git(dir: &Path, args: &[&str]) -> String {
    let out = tokio::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .await
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repo on `main` with two commits. Returns `(dir, first_sha, second_sha)`,
/// both abbreviated the way `rev-parse --short` prints them.
async fn repo_with_two_commits() -> (TempDir, String, String) {
    let tmp = TempDir::new().expect("tempdir");
    let repo = tmp.path();
    git(repo, &["init", "-q", "-b", "main"]).await;
    git(repo, &["config", "user.email", "t@example.com"]).await;
    git(repo, &["config", "user.name", "Test"]).await;
    std::fs::write(repo.join("f.txt"), "one").expect("seed file");
    git(repo, &["add", "."]).await;
    git(repo, &["commit", "-qm", "first"]).await;
    let first = git(repo, &["rev-parse", "--short", "HEAD"]).await;
    std::fs::write(repo.join("f.txt"), "two").expect("second write");
    git(repo, &["commit", "-qam", "second"]).await;
    let second = git(repo, &["rev-parse", "--short", "HEAD"]).await;
    (tmp, first, second)
}

/// The label a detached checkout reports is the label the recogniser accepts:
/// the round trip the IDE makes on every branch-aware request.
#[tokio::test]
async fn detached_label_round_trips_through_the_recogniser() {
    let (tmp, _first, second) = repo_with_two_commits().await;
    let repo = tmp.path();
    git(repo, &["checkout", "-q", "--detach"]).await;

    let label = branch::get_current_branch(repo).await.expect("label");
    assert_eq!(label, format!("HEAD@{second}"));
    assert_eq!(
        head_state(repo).await.expect("head state"),
        HeadState::Detached {
            short_sha: second.clone()
        }
    );

    assert_eq!(detached_label_sha(&label), Some(second.as_str()));
    assert!(is_detached_label(&label));
    verify_detached_label(repo, &label)
        .await
        .expect("the label the server just emitted must be honoured");
}

/// On a branch there is no label to recognise: the name is reported as-is.
#[tokio::test]
async fn a_branch_checkout_reports_its_name_not_a_label() {
    let (tmp, _first, _second) = repo_with_two_commits().await;
    let repo = tmp.path();

    let label = branch::get_current_branch(repo).await.expect("label");
    assert_eq!(label, "main");
    assert_eq!(detached_label_sha(&label), None);
    assert_eq!(
        head_state(repo).await.expect("head state").into_branch(),
        Ok("main".to_string())
    );
}

/// Only the exact shape the server emits is a label. Everything else — a path,
/// a ref, a revision expression, a real branch name — is not, and so still
/// goes through `validate_branch_name`, which refuses every one of these.
#[test]
fn only_the_emitted_shape_is_recognised() {
    let forged = [
        "HEAD@../../etc",
        "HEAD@../../etc/passwd",
        "HEAD@",
        "HEAD@zzzzzzz",
        "HEAD@abc123g",
        "HEAD@ABC1234",
        "HEAD@abc",
        "HEAD@abc1234/x",
        "HEAD@abc1234 ",
        " HEAD@abc1234",
        "HEAD@{1}",
        "HEAD@{upstream}",
        "HEAD@abc1234^",
        "HEAD@abc1234..main",
        "HEAD@-abc1234",
        "head@abc1234",
        "HEAD",
        "refs/heads/HEAD@abc1234",
        "main",
    ];
    for value in forged {
        assert_eq!(
            detached_label_sha(value),
            None,
            "{value:?} must not be recognised as a detached label"
        );
        if value.contains('@') {
            assert!(
                branch::validate_branch_name(value).is_err(),
                "{value:?} must still be refused as a branch name"
            );
        }
    }
    // 65 hex digits is longer than any object id git prints.
    assert_eq!(
        detached_label_sha(&format!("HEAD@{}", "a".repeat(65))),
        None
    );
    assert_eq!(
        detached_label_sha(&format!("HEAD@{}", "a".repeat(64))),
        Some("a".repeat(64).as_str())
    );
}

/// A well-formed label for a commit that is not HEAD is refused: the label can
/// only ever address the working copy as it stands, never another commit.
#[tokio::test]
async fn a_label_for_a_commit_that_is_not_head_is_refused() {
    let (tmp, first, second) = repo_with_two_commits().await;
    let repo = tmp.path();
    git(repo, &["checkout", "-q", "--detach"]).await;

    // `first` is a real commit in this repo — just not the one checked out.
    let err = verify_detached_label(repo, &format!("HEAD@{first}"))
        .await
        .expect_err("a sha that is not HEAD must be refused")
        .to_string();
    assert!(err.contains(&first) && err.contains(&second), "{err}");

    // A sha that names nothing at all.
    assert!(verify_detached_label(repo, "HEAD@0000000").await.is_err());
    // And not a label in the first place.
    assert!(verify_detached_label(repo, "HEAD@../../etc").await.is_err());
}

/// The label is refused once the working copy is back on a branch, even for
/// the very commit that branch points at.
#[tokio::test]
async fn a_label_is_refused_when_the_working_copy_is_on_a_branch() {
    let (tmp, _first, second) = repo_with_two_commits().await;
    let repo = tmp.path();

    let err = verify_detached_label(repo, &format!("HEAD@{second}"))
        .await
        .expect_err("main is checked out — nothing is detached")
        .to_string();
    assert!(err.contains("'main'"), "{err}");
}

/// Pushing "the current branch" from a detached HEAD says why it cannot run,
/// instead of handing git the label as a refspec
/// (`src refspec HEAD@<sha> does not match any`).
#[tokio::test]
async fn pushing_from_a_detached_head_says_there_is_no_branch() {
    let (tmp, _first, second) = repo_with_two_commits().await;
    let repo = tmp.path();
    let bare = tmp.path().join("remote.git");
    git(repo, &["init", "-q", "--bare", bare.to_str().unwrap()]).await;
    git(repo, &["remote", "add", "origin", bare.to_str().unwrap()]).await;
    git(repo, &["checkout", "-q", "--detach"]).await;

    for err in [
        push_pull::push_to_remote(repo, None).await,
        push_pull::force_push_to_remote(repo, None).await,
    ] {
        let err = err.expect_err("no branch to push").to_string();
        assert!(
            err.contains(&format!("detached HEAD at {second}")),
            "expected the detached-HEAD refusal, got: {err}"
        );
        assert!(!err.contains("refspec"), "git was still invoked: {err}");
    }
}
