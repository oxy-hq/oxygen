//! Write-back apply, against a real git repository in a temp dir.

use std::path::Path;

use chrono::Utc;
use oxy_app::server::experiment::apply::{
    ApplyCtx, ApplyError, apply_write_back, branch_name, user_slug,
};
use oxy_experiment::config::PriorSource;
use oxy_experiment::patch::{CoefficientEdit, ReplacedPrior};
use serde_json::json;

use super::fixtures::*;

const NAME: &str = "beverage_promo_pilot";
const FILE: &str = "views/store_days.view.yml";
const VIEW: &str = "\
name: store_days
measures:
  - name: beverages_revenue
    type: sum
    expr: beverages_revenue
    drivers:
      - measure: store_days.promo_redemptions
        direction: positive
";

fn edit() -> CoefficientEdit {
    CoefficientEdit {
        file_path: FILE.into(),
        target_measure: "store_days.beverages_revenue".into(),
        driver_measure: "store_days.promo_redemptions".into(),
        coefficient: 4.25,
        refs_entry: format!("experiment:{NAME}@3f9a0c41d2e7"),
        // The World Model held a fitted slope of 4.20 when the experiment was priced.
        replaces: Some(ReplacedPrior {
            value: 4.2,
            source: PriorSource::Fitted,
            edge_coefficient: None,
        }),
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// A repo whose `main` holds the view. Global git config is redirected into the
/// temp dir: `ensure_user_config` writes `--global` defaults when none exist.
fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().unwrap();
    // Outside the repo so the working tree stays clean; leaked for the process.
    let cfg = tempfile::TempDir::new().unwrap().keep();
    // SAFETY: nextest runs each test in its own process; set before any git call.
    unsafe {
        std::env::set_var("GIT_CONFIG_GLOBAL", cfg.join("gitconfig"));
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    }
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["config", "user.email", "test@oxy.local"]);
    git(dir.path(), &["config", "user.name", "Test"]);
    std::fs::create_dir_all(dir.path().join("views")).unwrap();
    std::fs::write(dir.path().join(FILE), VIEW).unwrap();
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    dir
}

async fn reported_run_with_patch(
    db: &sea_orm::DatabaseConnection,
    ws: uuid::Uuid,
    _rev: uuid::Uuid,
) {
    let run = seed_run(db, RunSeed::new(ws, NAME, "reported")).await;
    let patch = serde_json::to_string(&edit()).unwrap();
    assert!(
        insert_result_row(
            db,
            run,
            false,
            json!({ "write_back": { "status": "proposed" } }),
            Some(patch)
        )
        .await
    );
}

fn ctx<'a>(db: &'a sea_orm::DatabaseConnection, ws: uuid::Uuid, root: &'a Path) -> ApplyCtx<'a> {
    ApplyCtx {
        db,
        workspace: None,
        workspace_id: ws,
        root,
        name: NAME,
        user_id: None,
        user_slug: "ada",
        now: Utc::now(),
    }
}

#[tokio::test]
async fn apply_creates_a_branch_and_never_touches_main() {
    let repo = init_repo();
    let db = setup_db().await;
    let (ws, rev) = seed_workspace(&db, "apply").await;
    reported_run_with_patch(&db, ws, rev).await;
    let main_before = git(repo.path(), &["rev-parse", "main"]);
    let file_before = std::fs::read_to_string(repo.path().join(FILE)).unwrap();

    let out = apply_write_back(ctx(&db, ws, repo.path()))
        .await
        .expect("applies");

    assert!(
        out.branch
            .starts_with("ada/experiment-beverage-promo-pilot-"),
        "{}",
        out.branch
    );
    assert_eq!(out.file_path, FILE);
    // main: same commit, same file bytes, clean tree
    assert_eq!(git(repo.path(), &["rev-parse", "main"]), main_before);
    assert_eq!(
        std::fs::read_to_string(repo.path().join(FILE)).unwrap(),
        file_before
    );
    assert_eq!(git(repo.path(), &["status", "--porcelain"]).trim(), "");
    // the branch: exactly one commit ahead, carrying the edit and its provenance
    let on_branch = git(repo.path(), &["show", &format!("{}:{FILE}", out.branch)]);
    assert!(on_branch.contains("coefficient: 4.25"), "{on_branch}");
    assert!(
        on_branch.contains(&format!("experiment:{NAME}@3f9a0c41d2e7")),
        "{on_branch}"
    );
    assert!(on_branch.contains("form: linear"), "{on_branch}");
    assert_eq!(
        git(
            repo.path(),
            &["rev-list", "--count", &format!("main..{}", out.branch)]
        )
        .trim(),
        "1"
    );
    // the commit names the prior it replaces — a reviewer sees the model's old slope beside the new one
    let subject = git(repo.path(), &["log", "-1", "--format=%s", &out.branch]);
    assert!(
        subject.contains("replaces the fitted prior 4.20"),
        "{subject}"
    );
}

#[tokio::test]
async fn apply_refuses_when_the_edge_drifted_and_leaves_no_branch_behind() {
    let repo = init_repo();
    let db = setup_db().await;
    let (ws, rev) = seed_workspace(&db, "drift").await;
    reported_run_with_patch(&db, ws, rev).await;
    // main changed after registration: the edge is gone.
    std::fs::write(repo.path().join(FILE), "name: store_days\nmeasures: []\n").unwrap();
    git(repo.path(), &["commit", "-q", "-am", "drop the edge"]);

    let err = apply_write_back(ctx(&db, ws, repo.path()))
        .await
        .unwrap_err();
    assert!(matches!(err, ApplyError::Refused(_)), "{err:?}");
    assert_eq!(
        git(repo.path(), &["branch", "--format=%(refname:short)"]),
        "main\n",
        "no branch may exist"
    );
}

#[tokio::test]
async fn apply_is_refused_before_any_branch_for_a_run_that_cannot_apply() {
    let repo = init_repo();
    let db = setup_db().await;
    let (ws, _rev) = seed_workspace(&db, "refuse").await;

    assert!(matches!(
        apply_write_back(ctx(&db, ws, repo.path())).await,
        Err(ApplyError::NoSuchRun)
    ));

    let closing = seed_run(&db, RunSeed::new(ws, NAME, "closing")).await;
    assert!(matches!(
        apply_write_back(ctx(&db, ws, repo.path())).await,
        Err(ApplyError::NotReported { ref state }) if state == "closing"
    ));
    sea_orm::ConnectionTrait::execute_unprepared(
        &db,
        &format!("UPDATE experiment_runs SET state = 'reported' WHERE id = '{closing}'"),
    )
    .await
    .unwrap();
    // reported, but the write-back was refused: no patch, and the stored reason comes back.
    assert!(insert_result_row(
        &db,
        closing,
        false,
        json!({ "write_back": { "status": "refused", "reason": "the edge declares `form: log-log`" } }),
        None
    )
    .await);
    let err = apply_write_back(ctx(&db, ws, repo.path()))
        .await
        .unwrap_err();
    assert!(
        matches!(err, ApplyError::NoPatch { ref reason } if reason.contains("log-log")),
        "{err:?}"
    );
    assert_eq!(
        git(repo.path(), &["branch", "--format=%(refname:short)"]),
        "main\n"
    );
}

#[test]
fn branch_names_are_valid_and_carry_the_experiment() {
    let now = chrono::DateTime::parse_from_rfc3339("2026-10-14T09:05:03Z")
        .unwrap()
        .with_timezone(&Utc);
    let b = branch_name("ada", "beverage_promo_pilot", now);
    assert_eq!(b, "ada/experiment-beverage-promo-pilot-2026-10-14-090503");
    assert!(
        !b.contains("--"),
        "`--` is reserved for the worktree directory encoding"
    );
    assert_eq!(
        branch_name("ada", "a__b", now),
        "ada/experiment-a-b-2026-10-14-090503"
    );
    for (name, expect) in [
        ("pilot_", "pilot"),
        ("_x", "x"),
        ("a b", "a-b"),
        ("a..b", "a-b"),
        ("__", "experiment"),
        ("../..", "experiment"),
    ] {
        let b = branch_name("ada", name, now);
        assert_eq!(
            b,
            format!("ada/experiment-{expect}-2026-10-14-090503"),
            "{name:?}"
        );
        let out = std::process::Command::new("git")
            .args(["check-ref-format", "--branch", &b])
            .output()
            .unwrap();
        assert!(out.status.success(), "git rejects {b:?} (from {name:?})");
        assert!(!b.contains("--") && !b.contains(".."), "{b:?}");
    }
    assert_eq!(
        user_slug(Some("Ada.Lovelace@oxy.tech"), "x"),
        "ada-lovelace"
    );
    assert_eq!(user_slug(None, "Ada L"), "ada-l");
    assert_eq!(user_slug(None, ""), "user");
}

#[tokio::test]
async fn a_failed_switch_leaves_no_branch_behind() {
    let repo = init_repo();
    let db = setup_db().await;
    let (ws, rev) = seed_workspace(&db, "switchfail").await;
    reported_run_with_patch(&db, ws, rev).await;
    // `.worktrees` is a file: the ref is created, then the worktree cannot be.
    std::fs::write(repo.path().join(".worktrees"), "in the way").unwrap();

    let err = apply_write_back(ctx(&db, ws, repo.path()))
        .await
        .unwrap_err();
    assert!(matches!(err, ApplyError::Git(_)), "{err:?}");
    assert_eq!(
        git(repo.path(), &["branch", "--format=%(refname:short)"]),
        "main\n",
        "no branch may exist"
    );
}

#[tokio::test]
async fn apply_refuses_when_the_branch_already_exists_and_never_deletes_it() {
    let repo = init_repo();
    let db = setup_db().await;
    let (ws, rev) = seed_workspace(&db, "exists").await;
    reported_run_with_patch(&db, ws, rev).await;
    let now = Utc::now();
    let branch = branch_name("ada", NAME, now);
    // Someone else's branch at a different commit (a same-second double submit looks like this).
    git(repo.path(), &["checkout", "-q", "-b", "other"]);
    std::fs::write(repo.path().join("note.txt"), "x").unwrap();
    git(repo.path(), &["add", "-A"]);
    git(repo.path(), &["commit", "-q", "-m", "other work"]);
    git(repo.path(), &["branch", &branch]);
    git(repo.path(), &["checkout", "-q", "main"]);
    git(repo.path(), &["branch", "-D", "other"]);
    let tip = git(repo.path(), &["rev-parse", &branch]);

    let mut c = ctx(&db, ws, repo.path());
    c.now = now;
    let err = apply_write_back(c).await.unwrap_err();
    assert!(matches!(err, ApplyError::BranchExists { .. }), "{err:?}");
    assert_eq!(
        axum::response::IntoResponse::into_response(err).status(),
        axum::http::StatusCode::CONFLICT
    );
    assert_eq!(git(repo.path(), &["rev-parse", &branch]), tip, "tip intact");
}

#[tokio::test]
async fn apply_is_refused_for_an_aborted_run_with_a_row() {
    let repo = init_repo();
    let db = setup_db().await;
    let (ws, _rev) = seed_workspace(&db, "aborted").await;
    let run = seed_run(&db, RunSeed::new(ws, NAME, "aborted")).await;
    let patch = serde_json::to_string(&edit()).unwrap();
    assert!(
        insert_result_row(
            &db,
            run,
            true,
            json!({ "write_back": { "status": "proposed" } }),
            Some(patch)
        )
        .await
    );
    let err = apply_write_back(ctx(&db, ws, repo.path()))
        .await
        .unwrap_err();
    assert!(
        matches!(err, ApplyError::NotReported { ref state } if state == "aborted"),
        "{err:?}"
    );
    assert_eq!(
        git(repo.path(), &["branch", "--format=%(refname:short)"]),
        "main\n"
    );
}

#[tokio::test]
async fn apply_when_main_already_carries_the_edit_is_a_conflict_not_a_server_error() {
    let repo = init_repo();
    let db = setup_db().await;
    let (ws, rev) = seed_workspace(&db, "already").await;
    reported_run_with_patch(&db, ws, rev).await;
    // First apply on a branch, then merge it into main: HEAD now carries the edit.
    let first = apply_write_back(ctx(&db, ws, repo.path()))
        .await
        .expect("applies");
    git(repo.path(), &["merge", "-q", "--ff-only", &first.branch]);
    git(repo.path(), &["worktree", "prune"]);

    let mut c = ctx(&db, ws, repo.path());
    c.now = Utc::now() + chrono::Duration::seconds(5);
    let err = apply_write_back(c).await.unwrap_err();
    assert_eq!(
        axum::response::IntoResponse::into_response(err).status(),
        axum::http::StatusCode::CONFLICT
    );
}
