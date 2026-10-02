//! Integration-style coverage for the per-workspace git state fields on
//! `WorkspaceDetailsResponse`. The existing app-level test infrastructure
//! only exercises the CLI binary (see `build.rs` / `run.rs` / `test.rs`) —
//! spinning up a full HTTP server with auth + DB would require standing up
//! PostgreSQL fixtures, so these tests drive the response builder directly.
//! That gives us the same field-level coverage the plan calls for without
//! inventing a new test harness.

use oxy_app::api::workspaces::{
    GitCapabilities, GitMode, build_workspace_details_response,
    build_workspace_details_response_for_uninitialized_local, compute_workspace_storage_key,
};
use std::process::Command;
use tempfile::TempDir;
use uuid::Uuid;

/// Initialise a real git repository in `dir` via the system `git` binary.
/// `LocalGitService::is_git_repo` only checks for the presence of a `.git`
/// directory, but `get_default_branch` / `has_remote` shell out to real
/// `git` commands, so we need an actual repo on disk.
fn init_git_repo(dir: &std::path::Path) {
    let run = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("failed to invoke git");
        assert!(
            status.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&status.stderr)
        );
    };
    run(&["init", "-b", "main"]);
    run(&["config", "user.email", "oxy-test@example.com"]);
    run(&["config", "user.name", "Oxy Test"]);
    run(&["commit", "--allow-empty", "-m", "initial"]);
}

#[tokio::test]
async fn git_enabled_workspace_reports_local_mode() {
    // Clear GIT_REPOSITORY_URL so git_mode is driven solely by the repo's
    // own remote configuration (which we haven't added).
    // SAFETY: tests run in the same process; this is best-effort. We do not
    // restore the value because no other test in this file depends on it.
    unsafe {
        std::env::remove_var("GIT_REPOSITORY_URL");
    }

    let tmp = TempDir::new().expect("tempdir");
    init_git_repo(tmp.path());

    let workspace_id = Uuid::new_v4();
    let resp = build_workspace_details_response(
        workspace_id,
        "test-workspace",
        tmp.path(),
        false,
        true, // owns the fs; irrelevant here — the dir exists
        "owner".to_string(),
        workspace_id.to_string(),
    )
    .await
    .expect("builder returned error");

    let body = resp.0;
    assert_eq!(body.id, workspace_id);
    assert_eq!(body.name, "test-workspace");
    assert_eq!(body.current_user_role, "owner");
    assert_eq!(
        body.storage_key,
        workspace_id.to_string(),
        "cloud-mode storage_key must be the workspace UUID"
    );
    assert!(
        body.workspace_error.is_none(),
        "no workspace_error expected"
    );
    // .git exists, no remote, GIT_REPOSITORY_URL cleared → Local mode.
    assert_eq!(
        body.git_mode,
        GitMode::Local,
        "git_mode should be Local for a git repo without a remote"
    );
    assert!(body.capabilities.can_commit, "Local mode allows commits");
    assert!(
        !body.capabilities.can_push,
        "Local mode (no remote) cannot push"
    );
    assert_eq!(body.default_branch, "main");
    assert!(
        !body.protected_branches.is_empty(),
        "protected_branches should default to [default_branch]"
    );
    assert!(
        body.protected_branches.contains(&"main".to_string()),
        "protected_branches should contain default branch, got {:?}",
        body.protected_branches
    );
    assert!(
        !body.requires_local_setup,
        "requires_local_setup must be false here"
    );
    let branch = body.active_branch.expect("active_branch expected");
    assert_eq!(branch.name, "main");
}

/// A working copy on a detached HEAD (a CI `pull_request` checkout,
/// `git checkout <sha>`) has no branch. The details still fill `active_branch`
/// — with the `HEAD@<sha>` label the IDE sends back as `?branch=` — and say so
/// in `detached_head`, so the frontend shows "Detached at <sha>" without
/// parsing a label. On a branch the field is absent from the wire entirely.
#[tokio::test]
async fn a_detached_workspace_reports_the_label_and_its_sha() {
    unsafe {
        std::env::remove_var("GIT_REPOSITORY_URL");
    }
    let tmp = TempDir::new().expect("tempdir");
    init_git_repo(tmp.path());
    let details = |id: Uuid| {
        build_workspace_details_response(
            id,
            "ws",
            tmp.path(),
            false,
            true,
            "owner".to_string(),
            id.to_string(),
        )
    };

    let on_branch = details(Uuid::new_v4()).await.expect("on a branch").0;
    assert_eq!(on_branch.detached_head, None);
    let wire = serde_json::to_value(&on_branch).expect("serialises");
    assert!(wire.get("detached_head").is_none(), "{wire}");

    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(tmp.path())
            .output()
            .expect("failed to invoke git");
        assert!(out.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    git(&["checkout", "--detach"]);
    let sha = git(&["rev-parse", "--short", "HEAD"]);

    let detached = details(Uuid::new_v4()).await.expect("detached").0;
    assert_eq!(detached.detached_head.as_deref(), Some(sha.as_str()));
    assert_eq!(
        detached.active_branch.expect("active_branch").name,
        format!("HEAD@{sha}")
    );
    // Still a git workspace: the branch picker is the way out.
    assert!(detached.capabilities.can_switch_branch);
}

/// CLOUD: a missing working copy is a READINESS state, not a failure — git is
/// the source of truth, so the checkout is always re-clonable.
///
/// This must not be a `200`. It used to be, and during a k8s rolling update
/// that produced toast spam on the homepage: a `200` is invisible to the
/// ide-down detectors (which key on `502` + `x-oxy-required-role: ide`), it
/// carries `x-oxy-served-by: ide` so the FE's success interceptor RETIRED the
/// unavailable banner, and the FE toasted the error string then navigated away
/// — remounting the shell, refetching, and toasting again.
#[tokio::test]
async fn missing_workspace_directory_in_cloud_is_a_transient_503() {
    let tmp = TempDir::new().expect("tempdir");
    let missing = tmp.path().join("does-not-exist");
    assert!(!missing.exists());

    let workspace_id = Uuid::new_v4();
    let resp = build_workspace_details_response(
        workspace_id,
        "gone",
        &missing,
        false, // cloud
        true,  // this instance owns the workspace fs (ide / all)
        "admin".to_string(),
        workspace_id.to_string(),
    )
    .await
    .expect_err("cloud must report a missing working copy as transient, not 200");

    assert_eq!(
        resp.status(),
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "503 + Retry-After is the honest answer for 'not ready yet'"
    );
    assert_eq!(
        resp.headers()
            .get("x-oxy-unavailable")
            .expect("x-oxy-unavailable must be set so the FE can classify this"),
        "workspace-materializing"
    );
    assert_eq!(
        resp.headers()
            .get(axum::http::header::RETRY_AFTER)
            .expect("Retry-After must be set — this is a back-off, not a dead end"),
        "5"
    );
    // NOT the ide-down signal: the ide is reachable and serving its other
    // routes. Borrowing `x-oxy-required-role: ide` would conflate the two, and
    // a concurrent healthy ide response would flap the banner back off.
    assert!(
        resp.headers().get("x-oxy-required-role").is_none(),
        "must not masquerade as the ide-unreachable 502"
    );
}

/// SERVE REPLICA: a missing working copy is NORMAL here — a stateless replica
/// has no PVC. The workspace page's data half is `FleetOk`, so a replica serves
/// it from its own handler and the page still renders with git shown as
/// unavailable.
///
/// So this must stay a flagged `200`, NOT the materializing 503. Answering 503
/// here turns a benign degraded page into an unusable one: the shell would spin
/// on a condition that lasts as long as the ide outage. Cloud mode alone does
/// not make a missing directory transient — only owning the filesystem does.
#[tokio::test]
async fn missing_workspace_directory_on_a_serve_replica_still_degrades() {
    let tmp = TempDir::new().expect("tempdir");
    let missing = tmp.path().join("does-not-exist");

    let workspace_id = Uuid::new_v4();
    let resp = build_workspace_details_response(
        workspace_id,
        "degraded",
        &missing,
        false, // cloud
        false, // stateless serve replica — owns no workspace filesystem
        "admin".to_string(),
        workspace_id.to_string(),
    )
    .await
    .expect("a serve replica must degrade, not 503");

    let body = resp.0;
    assert_eq!(
        body.git_mode,
        GitMode::None,
        "git correctly reported unavailable while the ide is down"
    );
    assert!(
        body.workspace_error.is_none(),
        "a replica degrade carries NO error string: it is expected operation, not \
         a failure. The FE toasts `workspace_error` and redirects to the org root, \
         which would defeat the degrade — and the message named a server-side \
         filesystem path, which is our vocabulary and not something a user can act on"
    );
    assert!(!body.capabilities.can_commit);
    assert!(!body.capabilities.can_push);
}

/// LOCAL: no upstream to restore from, so a missing directory really is gone.
/// Keep the flagged `200` the FE renders as a toast.
#[tokio::test]
async fn missing_workspace_directory_in_local_reports_workspace_error() {
    let tmp = TempDir::new().expect("tempdir");
    let missing = tmp.path().join("does-not-exist");
    assert!(!missing.exists());

    let workspace_id = Uuid::new_v4();
    let resp = build_workspace_details_response(
        workspace_id,
        "gone",
        &missing,
        true,  // is_local
        false, // local: nothing to restore from, so it is genuinely gone
        "admin".to_string(),
        workspace_id.to_string(),
    )
    .await
    .expect("local keeps the flagged 200");

    let body = resp.0;
    assert_eq!(body.id, workspace_id);
    let err = body
        .workspace_error
        .as_ref()
        .expect("workspace_error should be set");
    assert!(
        !err.is_empty(),
        "workspace_error message should be non-empty"
    );
    assert_eq!(
        body.git_mode,
        GitMode::None,
        "git_mode must be None when dir missing"
    );
    assert!(
        !body.capabilities.can_commit,
        "no capabilities when dir missing"
    );
    assert!(
        !body.capabilities.can_push,
        "no capabilities when dir missing"
    );
    assert_eq!(body.default_branch, "main");
    assert_eq!(body.protected_branches, vec!["main".to_string()]);
    assert!(
        body.active_branch.is_none(),
        "active_branch should be None when dir missing"
    );
    assert!(
        !body.requires_local_setup,
        "requires_local_setup must be false here"
    );
}

#[tokio::test]
async fn local_mode_forces_git_mode_none_even_with_dot_git_present() {
    // User runs `oxy start` inside a directory that already has a .git
    // folder (e.g. the oxy source checkout). detect_git_mode would
    // otherwise report Local; we must force None so the frontend does
    // not light up UI for routes that are not mounted.
    let tmp = TempDir::new().expect("tempdir");
    init_git_repo(tmp.path());

    let workspace_id = Uuid::new_v4();
    let local_storage_key = compute_workspace_storage_key(workspace_id, Some(tmp.path()));
    let resp = build_workspace_details_response(
        workspace_id,
        "local-workspace",
        tmp.path(),
        true,  // is_local
        false, // local: a missing copy is never transient
        "owner".to_string(),
        local_storage_key.clone(),
    )
    .await
    .expect("builder returned error");

    let body = resp.0;
    assert_eq!(
        body.git_mode,
        GitMode::None,
        "local mode must force git_mode=None regardless of on-disk .git"
    );
    assert!(
        body.storage_key.starts_with("local:"),
        "local-mode storage_key must use the local: prefix, got {:?}",
        body.storage_key
    );
    assert_eq!(body.storage_key, local_storage_key);
    let expected_caps: GitCapabilities = GitMode::None.into();
    assert_eq!(
        body.capabilities, expected_caps,
        "capabilities must exactly match GitMode::None"
    );
    assert!(
        body.active_branch.is_none(),
        "active_branch must be None when git is disabled"
    );
    assert!(
        !body.requires_local_setup,
        "requires_local_setup must be false here"
    );
}

#[tokio::test]
async fn uninitialized_local_workspace_returns_requires_local_setup_true() {
    let workspace_id = Uuid::new_v4();
    let resp = build_workspace_details_response_for_uninitialized_local(
        workspace_id,
        "local",
        "owner".to_string(),
        "local:abc123def456".to_string(),
    );

    let body = resp.0;
    assert!(body.requires_local_setup);
    assert_eq!(body.git_mode, GitMode::None);
    assert!(body.workspace_error.is_none());
    assert_eq!(body.name, "local");
    assert_eq!(body.current_user_role, "owner");
    assert_eq!(body.storage_key, "local:abc123def456");
}

#[tokio::test]
async fn storage_key_differs_per_local_path_and_matches_uuid_for_cloud() {
    let workspace_id = Uuid::new_v4();
    let cloud = compute_workspace_storage_key(workspace_id, None);
    assert_eq!(
        cloud,
        workspace_id.to_string(),
        "cloud storage_key must equal the UUID"
    );

    let a = TempDir::new().expect("tempdir-a");
    let b = TempDir::new().expect("tempdir-b");
    let key_a = compute_workspace_storage_key(workspace_id, Some(a.path()));
    let key_b = compute_workspace_storage_key(workspace_id, Some(b.path()));
    assert!(key_a.starts_with("local:"));
    assert!(key_b.starts_with("local:"));
    assert_ne!(
        key_a, key_b,
        "different local paths must yield different storage_keys"
    );
    // Same path → same key (deterministic).
    assert_eq!(
        key_a,
        compute_workspace_storage_key(workspace_id, Some(a.path()))
    );
}

#[tokio::test]
async fn storage_key_resolves_symlinks() {
    let workspace_id = Uuid::new_v4();
    let target = TempDir::new().expect("tempdir-target");
    let parent = TempDir::new().expect("tempdir-parent");
    let link = parent.path().join("alias");

    #[cfg(unix)]
    std::os::unix::fs::symlink(target.path(), &link).expect("symlink");
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(target.path(), &link).expect("symlink");

    let direct = compute_workspace_storage_key(workspace_id, Some(target.path()));
    let via_link = compute_workspace_storage_key(workspace_id, Some(&link));
    assert_eq!(
        direct, via_link,
        "symlinked path must hash to the same storage_key as its target"
    );
}

#[tokio::test]
async fn storage_key_handles_missing_path() {
    let workspace_id = Uuid::new_v4();
    let tmp = TempDir::new().expect("tempdir");
    let missing = tmp.path().join("does-not-exist");
    assert!(!missing.exists());

    let key = compute_workspace_storage_key(workspace_id, Some(&missing));
    assert!(
        key.starts_with("local:"),
        "missing-path key must still use local: prefix, got {key:?}"
    );
    // Same missing path → same key (deterministic) even without canonicalize.
    assert_eq!(
        key,
        compute_workspace_storage_key(workspace_id, Some(&missing))
    );
}

// `#[serial]` because the test mutates process cwd; nextest's
// process-per-test isolation makes this safe on the project's default
// runner but a `cargo test` (threaded) run would race.
#[tokio::test]
#[serial_test::serial]
async fn storage_key_normalizes_relative_to_absolute() {
    let workspace_id = Uuid::new_v4();
    let tmp = TempDir::new().expect("tempdir");
    let original_cwd = std::env::current_dir().expect("get cwd");

    let parent = tmp.path().parent().expect("tempdir parent");
    let leaf = tmp.path().file_name().expect("tempdir name");
    std::env::set_current_dir(parent).expect("set cwd");

    let relative_key =
        compute_workspace_storage_key(workspace_id, Some(std::path::Path::new(leaf)));
    let absolute_key = compute_workspace_storage_key(workspace_id, Some(tmp.path()));

    // Restore before any assertion that could panic so a failure doesn't
    // leave the process cwd in the temp dir.
    std::env::set_current_dir(&original_cwd).expect("restore cwd");

    assert_eq!(
        relative_key, absolute_key,
        "relative and absolute paths to the same directory must hash equally"
    );
}
