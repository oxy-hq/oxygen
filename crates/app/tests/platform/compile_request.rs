//! `compile(workspace, branch | sha, kind)` and the status read beside it
//! (`oxy_app::server::compile_request`): the call a preview and a custom-app
//! staging build both make to get a branch compiled.
//!
//! What is pinned here is what makes it safe to answer from a pod with no
//! working copy: the branch head comes from GitHub, the compile is queued for
//! whoever can fetch the commit, a staging revision is never what the
//! workspace serves, a commit that is already compiled is not compiled again,
//! and a branch GitHub does not have is refused by name instead of guessed at.
//!
//! GitHub is a `wiremock` server reached through `GITHUB_API_URL`; the
//! workspace and its PAT connection come from [`super::compile_from_git`], and
//! `workspaces.path` names a directory that does not exist unless a test puts
//! a repository there. The process role is initialised for real, so on
//! `worker` and `serve` the workspace-path probe is armed.
//!
//! Run with:
//! `cargo nextest run -p oxy-app --test platform -E 'test(compile_request)'`

pub(super) mod fixture;

use oxy::workspace_fs_probe::leaks;
use oxy_app::server::compile_request::{self, NotFromGit, Refusal, Target};
use oxy_compile::RevisionKind;
use sea_orm::ActiveValue;
use wiremock::ResponseTemplate;

use fixture::{BRANCH, Fx, HEAD, SERVED, working_copy};

use super::compile_from_git::{tarball, workspace_tree};

fn needs_working_copy(refusal: Refusal) -> NotFromGit {
    match refusal {
        Refusal::NeedsWorkingCopy { why, .. } => why,
        other => panic!("expected a working-copy refusal, got {other:?}"),
    }
}

// ── A pushed branch, on a pod with no working copy ───────────────────────────

/// The rule a staging revision exists by: it is queued unpromoted and, once
/// compiled, the workspace still serves what it served before.
#[tokio::test]
async fn a_pushed_branch_compiles_from_git_and_never_moves_what_the_workspace_serves() {
    let fx = Fx::new(Some("worker")).await;
    fx.branch_is_at(BRANCH, HEAD).await;
    fx.serve_commit(HEAD, workspace_tree()).await;

    let asked = fx.compile(Target::Branch(BRANCH)).await.expect("queued");
    assert_eq!(asked.git_sha, HEAD);
    assert_eq!(asked.status, "pending");
    assert_eq!(asked.revision_id, None);

    let queued = fx.queued().await;
    assert_eq!(queued.len(), 1, "{queued:?}");
    let task = &queued[0];
    assert!(asked.task_id.is_some(), "this call queued it");
    assert_eq!(task.source_type, "compile_git", "any pod may claim it");
    assert_eq!(task.spec["from_git"], true);
    assert_eq!(task.spec["kind"], "staging");
    assert_eq!(task.spec["git_sha"], HEAD);
    assert_eq!(task.spec["branch"], BRANCH);
    // `promote` is omitted from the payload when false.
    assert_eq!(task.spec.get("promote"), None, "{}", task.spec);

    fx.drive_until_settled().await;

    let done = fx.status(HEAD).await;
    assert_eq!(done.status, "ready", "{done:?}");
    let revisions = fx.revisions_of(HEAD).await;
    assert_eq!(revisions.len(), 1, "{revisions:?}");
    assert_eq!(revisions[0].kind, "staging");
    assert_eq!(revisions[0].branch.as_deref(), Some(BRANCH));
    assert_eq!(done.revision_id, Some(revisions[0].revision_id));
    assert_eq!(
        fx.promoted().await,
        Some(fx.served),
        "a staging revision must never become what the workspace serves"
    );
    assert_eq!(leaks(), 0, "nothing reached for a working copy");
}

#[tokio::test]
async fn a_ready_revision_of_the_same_commit_is_reused_not_recompiled() {
    let fx = Fx::new(Some("serve")).await;
    fx.branch_is_at(BRANCH, HEAD).await;
    let staged = fx.ready_revision(HEAD, "staging").await;

    let again = fx.compile(Target::Branch(BRANCH)).await.expect("reused");
    assert_eq!(again.status, "ready");
    assert_eq!(again.revision_id, Some(staged));
    assert_eq!(again.task_id, None);
    assert!(fx.queued().await.is_empty(), "nothing was queued");
}

/// A pin reads rows by revision id, so the kind is irrelevant to it: the main
/// compile of a commit serves a preview of a branch at that commit.
#[tokio::test]
async fn a_ready_main_revision_of_the_commit_is_reused_too() {
    let fx = Fx::new(Some("serve")).await;
    fx.branch_is_at(BRANCH, SERVED).await;

    let reused = fx.compile(Target::Branch(BRANCH)).await.expect("reused");
    assert_eq!(reused.status, "ready");
    assert_eq!(reused.revision_id, Some(fx.served));
    assert!(fx.queued().await.is_empty(), "nothing was queued");
}

#[tokio::test]
async fn a_compile_already_queued_is_joined_not_doubled() {
    let fx = Fx::new(Some("serve")).await;
    fx.branch_is_at(BRANCH, HEAD).await;

    let first = fx.compile(Target::Branch(BRANCH)).await.expect("queued");
    let second = fx.compile(Target::Branch(BRANCH)).await.expect("joined");
    assert!(first.task_id.is_some());
    assert_eq!(second.status, "pending");
    assert_eq!(second.task_id, None, "the second call queued nothing");
    assert_eq!(fx.queued().await.len(), 1);
}

#[tokio::test]
async fn a_commit_is_compiled_by_its_sha_without_asking_where_a_branch_is() {
    let fx = Fx::new(Some("serve")).await;

    let asked = fx.compile(Target::Commit(&HEAD.to_uppercase())).await;
    let asked = asked.expect("queued");
    assert_eq!(asked.git_sha, HEAD, "the SHA is normalised");
    let queued = fx.queued().await;
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].source_type, "compile_git");
    assert_eq!(queued[0].spec["kind"], "staging");
    assert_eq!(queued[0].spec.get("branch"), None);
    assert_eq!(fx.github_requests().await, 0);

    let refused = fx.compile(Target::Commit("feat/x")).await.unwrap_err();
    assert!(matches!(refused, Refusal::NotACommit(_)), "{refused:?}");
}

// ── What is refused ──────────────────────────────────────────────────────────

#[tokio::test]
async fn only_staging_is_compiled_through_this_call() {
    let fx = Fx::new(Some("serve")).await;
    fx.branch_is_at(BRANCH, HEAD).await;
    let workspace = fx.workspace().await;

    for kind in [RevisionKind::Main, RevisionKind::Draft] {
        let refused =
            compile_request::compile(&fx.db, &workspace, Target::Branch(BRANCH), kind).await;
        let refused = refused.unwrap_err();
        assert!(
            matches!(refused, Refusal::UnsupportedKind(_)),
            "{kind:?}: {refused:?}"
        );
        assert_eq!(refused.status(), 400);
    }
    assert!(fx.queued().await.is_empty());
    assert_eq!(
        fx.github_requests().await,
        0,
        "refused before asking GitHub"
    );
}

/// A pod with no working copy can only compile what GitHub has. Each reason it
/// cannot is named, and nothing is queued.
#[tokio::test]
async fn what_github_cannot_serve_is_refused_by_name_on_a_pod_with_no_working_copy() {
    let fx = Fx::new(Some("serve")).await;
    fx.github_answers(BRANCH, ResponseTemplate::new(404)).await;
    fx.github_answers("locked", ResponseTemplate::new(401))
        .await;

    let unpushed = fx.compile(Target::Branch(BRANCH)).await.unwrap_err();
    assert_eq!(unpushed.status(), 409);
    let message = unpushed.to_string();
    assert!(message.contains("[branch_not_pushed]"), "{message}");
    assert!(message.contains("push the branch"), "{message}");
    assert!(matches!(
        needs_working_copy(unpushed),
        NotFromGit::BranchNotFound { .. }
    ));

    let denied = fx.compile(Target::Branch("locked")).await.unwrap_err();
    assert_eq!(needs_working_copy(denied).code(), "github_denied");

    fx.edit_workspace(|w| w.git_namespace_id = ActiveValue::Set(None))
        .await;
    let unlinked = fx.compile(Target::Branch(BRANCH)).await.unwrap_err();
    assert_eq!(needs_working_copy(unlinked), NotFromGit::NoConnection);

    fx.edit_workspace(|w| w.current_revision_id = ActiveValue::Set(None))
        .await;
    let uncompiled = fx.compile(Target::Branch(BRANCH)).await.unwrap_err();
    assert_eq!(needs_working_copy(uncompiled), NotFromGit::NothingCompiled);

    fx.edit_workspace(|w| w.git_remote_url = ActiveValue::Set(None))
        .await;
    let no_remote = fx.compile(Target::Branch(BRANCH)).await.unwrap_err();
    assert_eq!(needs_working_copy(no_remote), NotFromGit::NoRemote);

    assert!(fx.queued().await.is_empty(), "a refusal queues nothing");
    assert_eq!(leaks(), 0, "a refusal does not go looking for a disk");
}

#[tokio::test]
async fn a_bad_name_is_refused_before_github_is_asked() {
    let fx = Fx::new(Some("serve")).await;

    for (branch, status) in [("../etc", 400), ("feat--x", 400), ("HEAD@abc1234", 409)] {
        let refused = fx.compile(Target::Branch(branch)).await.unwrap_err();
        assert_eq!(refused.status(), status, "{branch}: {refused:?}");
    }
    assert_eq!(fx.github_requests().await, 0);
}

// ── A pod that holds the working copy ────────────────────────────────────────

/// The case that must not change while the node with the files exists: a
/// branch that was never pushed compiles from that node's working copy.
#[tokio::test]
async fn a_branch_only_the_working_copy_has_is_compiled_from_it_where_there_is_one() {
    let fx = Fx::new(None).await;
    let (repo, _pushed, local_only) = working_copy();
    let root = repo.path().to_string_lossy().into_owned();
    fx.edit_workspace(|w| w.path = ActiveValue::Set(Some(root)))
        .await;
    fx.github_answers("local-only", ResponseTemplate::new(404))
        .await;
    fx.github_answers("nowhere", ResponseTemplate::new(404))
        .await;

    let asked = fx.compile(Target::Branch("local-only")).await;
    let asked = asked.expect("compiled from the working copy");
    assert_eq!(asked.git_sha, local_only);
    let queued = fx.queued().await;
    assert_eq!(queued.len(), 1, "{queued:?}");
    assert_eq!(
        queued[0].source_type, "compile",
        "only the node with the files may claim a working-copy compile"
    );
    assert_eq!(queued[0].spec.get("from_git"), None, "{}", queued[0].spec);
    assert_eq!(queued[0].spec["kind"], "staging");
    assert_eq!(queued[0].spec.get("promote"), None);

    // In neither place: unknown, and it says GitHub was asked.
    let unknown = fx.compile(Target::Branch("nowhere")).await.unwrap_err();
    assert_eq!(unknown.status(), 404);
    assert!(unknown.to_string().contains("not on GitHub"), "{unknown}");
}

/// Git is the source wherever it can be: the node with the files does not
/// compile its own copy of a branch GitHub has, so the same request compiles
/// the same commit on every pod.
#[tokio::test]
async fn a_pushed_branch_is_compiled_from_git_even_where_there_is_a_working_copy() {
    let fx = Fx::new(None).await;
    let (repo, local_head, _) = working_copy();
    let root = repo.path().to_string_lossy().into_owned();
    fx.edit_workspace(|w| w.path = ActiveValue::Set(Some(root)))
        .await;
    fx.branch_is_at(BRANCH, HEAD).await;
    assert_ne!(local_head, HEAD);

    let asked = fx.compile(Target::Branch(BRANCH)).await.expect("queued");
    assert_eq!(asked.git_sha, HEAD, "GitHub's head, not the local one");
    let queued = fx.queued().await;
    assert_eq!(queued[0].source_type, "compile_git");
    assert!(
        !repo.path().join(".git/worktrees").exists(),
        "no worktree is made for a branch that is fetched"
    );
}

/// An outage is not "the branch is not on GitHub". Falling back to the working
/// copy here would compile whatever commit that node happens to have and call
/// it the branch.
#[tokio::test]
async fn github_being_down_is_a_retryable_refusal_and_never_a_fallback() {
    let fx = Fx::new(None).await;
    let (repo, _, _) = working_copy();
    let root = repo.path().to_string_lossy().into_owned();
    fx.edit_workspace(|w| w.path = ActiveValue::Set(Some(root)))
        .await;
    fx.github_answers(BRANCH, ResponseTemplate::new(502)).await;
    fx.github_answers("local-only", ResponseTemplate::new(429))
        .await;

    for branch in [BRANCH, "local-only"] {
        let refused = fx.compile(Target::Branch(branch)).await.unwrap_err();
        assert!(
            matches!(refused, Refusal::GitHubUnavailable { .. }),
            "{branch}: {refused:?}"
        );
        assert_eq!(refused.status(), 503);
    }
    assert!(fx.queued().await.is_empty());
}

// ── The status read ──────────────────────────────────────────────────────────

/// Fetching the commit happens before a revision row is written, so a compile
/// that fails there leaves no row. It must still read as failed, with the
/// reason, or the caller polls a dead compile until its own timeout.
#[tokio::test]
async fn a_compile_that_fails_before_it_writes_a_revision_is_reported_failed_with_its_reason() {
    let fx = Fx::new(Some("worker")).await;
    fx.branch_is_at(BRANCH, HEAD).await;
    // A tree with no config.yml at the workspace root is refused once fetched.
    fx.serve_commit(HEAD, tarball(&[("README.md", "not a workspace")]))
        .await;

    assert_eq!(fx.status(HEAD).await.status, "pending", "nothing asked yet");
    fx.compile(Target::Branch(BRANCH)).await.expect("queued");
    assert_eq!(fx.status(HEAD).await.status, "pending", "queued");
    fx.drive_until_settled().await;

    assert!(fx.revisions_of(HEAD).await.is_empty(), "no revision row");
    let failed = fx.status(HEAD).await;
    assert_eq!(failed.status, "failed", "{failed:?}");
    assert_eq!(failed.revision_id, None);
    let reason = failed.error.expect("a reason");
    assert!(
        reason.contains("compile from git failed [no_config]"),
        "{reason}"
    );

    // Asking again is the retry: it queues a fresh compile, and the status
    // follows that one rather than the failure before it.
    let again = fx.compile(Target::Branch(BRANCH)).await.expect("queued");
    assert!(again.task_id.is_some());
    assert_eq!(fx.status(HEAD).await.status, "pending");
}

/// A staging request joins a compile of the commit that is already queued,
/// whatever its kind. When the one it joined is a main compile that then fails
/// before writing a revision, the staging caller is waiting on that task and
/// must be told, not left at `pending`.
#[tokio::test]
async fn a_joined_main_compile_that_fails_before_a_revision_is_reported_to_the_staging_caller() {
    let fx = Fx::new(Some("worker")).await;
    fx.branch_is_at(BRANCH, HEAD).await;
    fx.serve_commit(HEAD, tarball(&[("README.md", "not a workspace")]))
        .await;
    // What the periodic check queues when a branch head moved.
    oxy_app::server::compile_git::enqueue(
        &fx.db,
        fx.ws,
        HEAD,
        Some("main"),
        RevisionKind::Main,
        true,
    )
    .await
    .expect("queue the main compile");

    let joined = fx.compile(Target::Branch(BRANCH)).await.expect("joined");
    assert_eq!(joined.status, "pending");
    assert_eq!(joined.task_id, None, "it queued nothing of its own");
    fx.drive_until_settled().await;

    assert_eq!(fx.queued().await.len(), 1, "only the main compile ran");
    let failed = fx.status(HEAD).await;
    assert_eq!(failed.status, "failed", "{failed:?}");
    let reason = failed.error.expect("a reason");
    assert!(reason.contains("[no_config]"), "{reason}");
}
