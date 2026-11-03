//! A push to a branch a custom-app environment's semantic plane names
//! (`env_pins::on_branch_push`, reached from `push::handle`).
//!
//! What a live database shows: only live, non-production environments whose
//! `planes` name the pushed branch (in either wire form) are written; the
//! commit is stored as the waiting `semantic_git_sha` while the revision being
//! served stays put until the compile lands; a head that is already compiled
//! moves the pin at once; a compile that finishes for a commit the branch has
//! since moved past moves nothing; and a deleted branch touches no pin.
//!
//! Run with:
//! `cargo nextest run -p oxy-app --test platform -E 'test(env_pins)'`

use oxy_app::server::compile_reconcile::env_pins::{move_after_compile, on_branch_push};
use oxy_app::server::compile_reconcile::push::{
    self, Handled, PushCommit, PushEvent, PushRepository,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{NEW, OLD, pin_of, seed_app, seed_org, wait_on};
use crate::compile_request::fixture::{BRANCH, Fx, HEAD};

fn event(branch: &str, after: &str, repo: &str) -> PushEvent {
    PushEvent {
        git_ref: format!("refs/heads/{branch}"),
        deleted: false,
        after: after.into(),
        repository: PushRepository {
            full_name: repo.into(),
        },
        commits: Vec::<PushCommit>::new(),
    }
}

fn push_to(branch: &str, after: &str) -> PushEvent {
    event(branch, after, "acme/analytics")
}

/// Set the `planes` of `app`'s `name` environment.
async fn set_planes(db: &DatabaseConnection, app: Uuid, name: &str, planes: Value) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE app_environments SET planes = $3::jsonb WHERE app_id = $1 AND name = $2",
        [app.into(), name.into(), planes.to_string().into()],
    ))
    .await
    .expect("set planes");
}

/// An extra environment of `app`, with `planes`.
async fn add_env(db: &DatabaseConnection, app: Uuid, name: &str, kind: &str, planes: Value) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO app_environments (app_id, name, kind, planes) VALUES ($1, $2, $3, $4::jsonb)",
        [
            app.into(),
            name.into(),
            kind.into(),
            planes.to_string().into(),
        ],
    ))
    .await
    .expect("add environment");
}

/// The waiting sha of `app`'s `name` environment.
async fn sha_of(db: &DatabaseConnection, app: Uuid, name: &str) -> Option<String> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT semantic_git_sha FROM app_environments WHERE app_id = $1 AND name = $2",
            [app.into(), name.into()],
        ))
        .await
        .expect("read sha")
        .expect("environment exists");
    row.try_get("", "semantic_git_sha").expect("column")
}

fn on_feature_branch() -> Value {
    json!({ "semantic": format!("branch:{BRANCH}") })
}

#[tokio::test]
async fn push_to_pinned_branch_marks_env_sha() {
    let fx = Fx::new(Some("serve")).await;
    fx.branch_is_at(BRANCH, HEAD).await;
    let org = seed_org(&fx.db).await;
    let pinned = seed_app(&fx.db, org, fx.ws).await;
    let elsewhere = seed_app(&fx.db, org, fx.ws).await;
    let unset = seed_app(&fx.db, org, fx.ws).await;
    let leaving = seed_app(&fx.db, org, fx.ws).await;
    set_planes(&fx.db, pinned, "staging", on_feature_branch()).await;
    set_planes(
        &fx.db,
        elsewhere,
        "staging",
        json!({"semantic": "branch:other"}),
    )
    .await;
    set_planes(&fx.db, leaving, "staging", on_feature_branch()).await;
    fx.db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE app_environments SET deleting_at = now() WHERE app_id = $1",
            [leaving.into()],
        ))
        .await
        .expect("mark deleting");
    // It was serving an older model, which must keep serving until HEAD lands.
    wait_on(&fx.db, pinned, OLD).await;
    let old = Uuid::new_v4();
    move_after_compile(&fx.db, fx.ws, OLD, old)
        .await
        .expect("pin old");
    add_env(
        &fx.db,
        pinned,
        "production",
        "production",
        on_feature_branch(),
    )
    .await;

    let queued = on_branch_push(&fx.db, &push_to(BRANCH, HEAD), BRANCH)
        .await
        .expect("push");

    assert_eq!(queued, 1, "one compile for the one workspace");
    assert_eq!(pin_of(&fx.db, pinned).await, (Some(old), Some(HEAD.into())));
    assert_eq!(pin_of(&fx.db, elsewhere).await, (None, None));
    assert_eq!(pin_of(&fx.db, unset).await, (None, None));
    assert_eq!(pin_of(&fx.db, leaving).await, (None, None));
    assert_eq!(sha_of(&fx.db, pinned, "production").await, None);
    let tasks = fx.queued().await;
    assert_eq!(tasks.len(), 1, "{tasks:?}");
    assert_eq!(tasks[0].spec["git_sha"], HEAD);
    assert_eq!(tasks[0].spec["kind"], "staging");
}

#[tokio::test]
async fn a_plane_written_as_an_object_matches_too() {
    let fx = Fx::new(Some("serve")).await;
    fx.branch_is_at(BRANCH, HEAD).await;
    let org = seed_org(&fx.db).await;
    let app = seed_app(&fx.db, org, fx.ws).await;
    let wrong_mode = seed_app(&fx.db, org, fx.ws).await;
    set_planes(
        &fx.db,
        app,
        "staging",
        json!({"semantic": {"mode": "branch", "branch": BRANCH}}),
    )
    .await;
    set_planes(
        &fx.db,
        wrong_mode,
        "staging",
        json!({"semantic": {"mode": "build-pin", "branch": BRANCH}}),
    )
    .await;

    let queued = on_branch_push(&fx.db, &push_to(BRANCH, HEAD), BRANCH)
        .await
        .expect("push");

    assert_eq!(queued, 1);
    assert_eq!(pin_of(&fx.db, app).await, (None, Some(HEAD.into())));
    assert_eq!(pin_of(&fx.db, wrong_mode).await, (None, None));
}

#[tokio::test]
async fn a_head_that_is_already_compiled_moves_the_pin_at_once() {
    let fx = Fx::new(Some("serve")).await;
    fx.branch_is_at(BRANCH, HEAD).await;
    let org = seed_org(&fx.db).await;
    let app = seed_app(&fx.db, org, fx.ws).await;
    set_planes(&fx.db, app, "staging", on_feature_branch()).await;
    let revision = fx.ready_revision(HEAD, "staging").await;

    let queued = on_branch_push(&fx.db, &push_to(BRANCH, HEAD), BRANCH)
        .await
        .expect("push");

    assert_eq!(queued, 0, "nothing to compile");
    assert!(fx.queued().await.is_empty());
    assert_eq!(
        pin_of(&fx.db, app).await,
        (Some(revision), Some(HEAD.into()))
    );
}

#[tokio::test]
async fn a_push_to_another_repository_touches_no_pin() {
    let fx = Fx::new(Some("serve")).await;
    let org = seed_org(&fx.db).await;
    let app = seed_app(&fx.db, org, fx.ws).await;
    set_planes(&fx.db, app, "staging", on_feature_branch()).await;

    let queued = on_branch_push(&fx.db, &event(BRANCH, HEAD, "other/repo"), BRANCH)
        .await
        .expect("push");

    assert_eq!(queued, 0);
    assert_eq!(pin_of(&fx.db, app).await, (None, None));
    assert!(fx.queued().await.is_empty());
}

/// Not `OLD`: that is the commit the fixture's workspace already serves, which
/// a compile would reuse rather than queue.
const FIRST: &str = "3333333333333333333333333333333333333333";

#[tokio::test]
async fn stale_compile_does_not_move_pin() {
    let fx = Fx::new(Some("serve")).await;
    let org = seed_org(&fx.db).await;
    let app = seed_app(&fx.db, org, fx.ws).await;
    set_planes(&fx.db, app, "staging", on_feature_branch()).await;
    fx.branch_is_at(BRANCH, FIRST).await;
    on_branch_push(&fx.db, &push_to(BRANCH, FIRST), BRANCH)
        .await
        .expect("first push");
    fx.github.reset().await;
    fx.branch_is_at(BRANCH, NEW).await;
    on_branch_push(&fx.db, &push_to(BRANCH, NEW), BRANCH)
        .await
        .expect("second push");

    // The first commit's compile finishes after the second was pushed.
    let stale = move_after_compile(&fx.db, fx.ws, FIRST, Uuid::new_v4())
        .await
        .expect("late move");

    assert_eq!(stale, 0);
    assert_eq!(pin_of(&fx.db, app).await, (None, Some(NEW.into())));
    let landed = Uuid::new_v4();
    move_after_compile(&fx.db, fx.ws, NEW, landed)
        .await
        .expect("move");
    assert_eq!(pin_of(&fx.db, app).await, (Some(landed), Some(NEW.into())));
}

/// A delivery for an older commit (redelivered, or out of order) must not roll
/// the pin back: the commit followed is the one the branch is at now.
#[tokio::test]
async fn a_redelivered_older_push_cannot_roll_the_pin_back() {
    let fx = Fx::new(Some("serve")).await;
    let org = seed_org(&fx.db).await;
    let app = seed_app(&fx.db, org, fx.ws).await;
    set_planes(&fx.db, app, "staging", on_feature_branch()).await;
    fx.branch_is_at(BRANCH, NEW).await;

    on_branch_push(&fx.db, &push_to(BRANCH, FIRST), BRANCH)
        .await
        .expect("push");

    assert_eq!(
        pin_of(&fx.db, app).await,
        (None, Some(NEW.into())),
        "waiting on the branch's head, not on the delivery's `after`"
    );
    let tasks = fx.queued().await;
    assert_eq!(tasks.len(), 1, "{tasks:?}");
    assert_eq!(tasks[0].spec["git_sha"], NEW);
}

#[tokio::test]
async fn push_handle_ignores_deleted_branch_for_env_pins() {
    let fx = Fx::new(Some("serve")).await;
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::set_var("OXY_COMPILE_RECONCILE", "1") };
    let org = seed_org(&fx.db).await;
    let app = seed_app(&fx.db, org, fx.ws).await;
    set_planes(&fx.db, app, "staging", on_feature_branch()).await;
    let mut deleted = push_to(BRANCH, &"0".repeat(40));
    deleted.deleted = true;

    let handled = push::handle(&fx.db, &deleted).await.expect("handle");

    assert!(matches!(handled, Handled::Ignored(_)), "{handled:?}");
    assert_eq!(pin_of(&fx.db, app).await, (None, None));
    assert!(fx.queued().await.is_empty());
}

#[tokio::test]
async fn push_handle_counts_env_pin_compiles_as_queued() {
    let fx = Fx::new(Some("serve")).await;
    fx.branch_is_at(BRANCH, HEAD).await;
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::set_var("OXY_COMPILE_RECONCILE", "1") };
    let org = seed_org(&fx.db).await;
    let app = seed_app(&fx.db, org, fx.ws).await;
    set_planes(&fx.db, app, "staging", on_feature_branch()).await;

    let handled = push::handle(&fx.db, &push_to(BRANCH, HEAD))
        .await
        .expect("handle");

    // The workspace's default branch is `main`, so it does not match; only the
    // environment's compile is queued.
    assert_eq!(
        handled,
        Handled::Matched {
            matched: 0,
            queued: 1
        }
    );
    assert_eq!(pin_of(&fx.db, app).await, (None, Some(HEAD.into())));
}
