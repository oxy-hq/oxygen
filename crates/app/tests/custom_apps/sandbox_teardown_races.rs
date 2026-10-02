//! Two teardown runs of one sandbox (review S1): a late run removes nothing
//! once an earlier one has finished — not what landed under the name since,
//! and nothing of a sandbox created again; a run that cannot take the
//! sandbox's lock removes nothing; a deleted app's sandbox is still torn
//! down; and a secret another run removed is not a failure.

use agentic_core::delegation::TaskOutcome;
use entity::apps;
use oxy::service::secret_manager::SecretManagerService;
use oxy_app::server::api::custom_apps_migrations::read_ledger;
use oxy_app::server::api::custom_apps_sandboxes::lock::SandboxLock;
use oxy_app::server::api::custom_apps_sandboxes::teardown::{self, SandboxTeardownTask};
use oxy_app::server::api::custom_apps_sandboxes::{TeardownReason, ops};
use oxy_app::server::api::custom_apps_secrets::delete_named_secrets;
use sea_orm::EntityTrait;

use crate::custom_app_functions_fixture::seeded_tenant;
use crate::sandbox_teardown::{AIRHOUSE_APP, sandbox};
use crate::sandbox_teardown_task::{
    furnished_sandbox, migrated_sibling, objects, point_airhouse_nowhere, published, put_object,
    queued, row, run_queued, secret, teardowns_queued, use_scratch_homes,
};

/// **A late run never removes what is under the name once an earlier run
/// has finished** (review S1). While a teardown is on its way a second
/// `DELETE` queues nothing; a second run all the same — a task claimed
/// twice — finds the row gone while the app exists, and stops: an object
/// that landed under the name since is not its to remove. Nor is anything
/// of the sandbox created again under the name: its silo, its secret and
/// its Airhouse sibling's ledger are all still there.
#[tokio::test]
async fn a_late_run_removes_nothing_once_an_earlier_run_has_finished() {
    let tmp = use_scratch_homes();
    // A run that reached for Airhouse would fail here, not pass.
    point_airhouse_nowhere();
    let t = seeded_tenant().await;
    let (app, build) = published(&t, AIRHOUSE_APP).await;
    furnished_sandbox(&t, &app, build, "a1").await;
    let a1 = sandbox("a1");
    let delete = || ops::delete(&t.db, &app, &a1, Some(t.guest_id), TeardownReason::Deleted);

    let first = delete().await.expect("delete dev-a1");
    assert!(first.queued && first.was_active, "{first:?}");
    let second = delete().await.expect("delete dev-a1 again");
    assert_eq!(second.run_id, first.run_id, "the run already on its way");
    assert!(!second.queued && !second.was_active, "{second:?}");
    assert_eq!(teardowns_queued(&t.db, app.id).await, 1);

    // The second run, built by hand from the first's payload.
    let payload = queued(&t.db, &first.run_id).await;
    let late = SandboxTeardownTask {
        marked_at_micros: 1,
        ..SandboxTeardownTask::from_spec(&payload).expect("a teardown payload")
    };
    let outcome = run_queued(&t.db, &first.run_id).await;
    assert!(matches!(outcome, TaskOutcome::Done { .. }), "{outcome:?}");
    assert_eq!(row(&t.db, app.id, "dev-a1").await, None);

    // Row gone, app there: whatever is under the name is nobody's to remove.
    put_object(app.id, &a1, "uploads/late.bin").await;
    let skipped = teardown::run(&t.db, &late).await.expect("a late run");
    assert!(skipped.contains("already torn down"), "{skipped}");
    assert!(skipped.contains("nothing was removed"), "{skipped}");
    assert_eq!(objects(app.id, &a1).await, 1);

    // Created again, published to and migrated: the late run leaves it all.
    furnished_sandbox(&t, &app, build, "a1").await;
    let home = migrated_sibling(&t, &app, build, "a1").await;
    let skipped = teardown::run(&t.db, &late).await.expect("a late run");
    assert!(skipped.contains("is active"), "{skipped}");
    assert!(skipped.contains("nothing was removed"), "{skipped}");
    assert_eq!(row(&t.db, app.id, "dev-a1").await, Some(false));
    assert_eq!(objects(app.id, &a1).await, 2, "late.bin and its own");
    assert_eq!(secret(&app, "dev-a1").await.as_deref(), Some("a1"));
    let ledger = read_ledger(&t.db, app.id, "airhouse", home.target())
        .await
        .expect("ledger");
    assert_eq!(ledger.len(), 2, "its sibling's ledger is untouched");
    let _ = std::fs::remove_dir_all(tmp);
}

/// Two runs of one sandbox never overlap: a run that cannot take the
/// sandbox's lock removes nothing and fails, leaving the row `deleting`;
/// once the lock is free the same run tears the sandbox down. Another
/// sandbox's lock is its own.
#[tokio::test]
async fn a_run_that_cannot_take_the_sandboxs_lock_removes_nothing() {
    let tmp = use_scratch_homes();
    let t = seeded_tenant().await;
    let (app, build) = published(&t, "sbx-lock").await;
    furnished_sandbox(&t, &app, build, "a1").await;
    let run_id = ops::begin_delete(
        &t.db,
        &app,
        &sandbox("a1"),
        Some(t.guest_id),
        TeardownReason::Deleted,
    )
    .await
    .expect("delete dev-a1");
    let task =
        SandboxTeardownTask::from_spec(&queued(&t.db, &run_id).await).expect("a teardown payload");

    let held = SandboxLock::try_acquire(&t.db, app.id, "dev-a1")
        .await
        .expect("the lock")
        .expect("nobody holds it yet");
    let refused = teardown::run_with(&t.db, &task, &[])
        .await
        .expect_err("the sandbox is locked");
    assert!(refused.contains("still running"), "{refused}");
    assert_eq!(row(&t.db, app.id, "dev-a1").await, Some(true));
    assert_eq!(objects(app.id, &sandbox("a1")).await, 1);
    assert_eq!(secret(&app, "dev-a1").await.as_deref(), Some("a1"));
    let other = SandboxLock::try_acquire(&t.db, app.id, "dev-b2")
        .await
        .expect("the lock");
    assert!(other.is_some(), "dev-b2's lock is not dev-a1's");

    held.release().await;
    let done = teardown::run_with(&t.db, &task, &[])
        .await
        .expect("torn down once the lock is free");
    assert!(done.contains("dev-a1 torn down"), "{done}");
    assert_eq!(row(&t.db, app.id, "dev-a1").await, None);
    assert_eq!(objects(app.id, &sandbox("a1")).await, 0);
    let _ = std::fs::remove_dir_all(tmp);
}

/// Deleting an app outright takes its sandboxes' rows with it and leaves
/// their homes. With no row **and no app**, a teardown still removes them:
/// that is the one case where an absent row means "go on".
#[tokio::test]
async fn a_teardown_of_a_deleted_apps_sandbox_still_removes_its_homes() {
    let tmp = use_scratch_homes();
    let t = seeded_tenant().await;
    let (app, build) = published(&t, "sbx-gone").await;
    furnished_sandbox(&t, &app, build, "a1").await;
    apps::Entity::delete_by_id(app.id)
        .exec(&t.db)
        .await
        .expect("delete the app");
    assert_eq!(
        row(&t.db, app.id, "dev-a1").await,
        None,
        "its rows went too"
    );

    let task = SandboxTeardownTask {
        app_id: app.id,
        app_slug: app.slug.clone(),
        org_id: app.org_id,
        workspace_id: app.project_id,
        environment: "dev-a1".into(),
        reason: "deleted".into(),
        marked_at_micros: 1,
    };
    let done = teardown::run(&t.db, &task).await.expect("torn down");
    assert!(done.contains("its row was already gone"), "{done}");
    assert_eq!(objects(app.id, &sandbox("a1")).await, 0);
    assert_eq!(secret(&app, "dev-a1").await, None);
    let _ = std::fs::remove_dir_all(tmp);
}

/// A secret another run removed between the listing and the delete is
/// deleted, not a failure: a repeated teardown must not end `Failed`.
#[tokio::test]
async fn deleting_a_secret_that_is_already_gone_is_not_a_failure() {
    let tmp = use_scratch_homes();
    let t = seeded_tenant().await;
    let (app, _build) = published(&t, "sbx-secret").await;
    SecretManagerService::new(app.project_id)
        .set_app_secret_in(&t.db, app.id, Some("dev-a1"), "TOKEN", "a1", t.guest_id)
        .await
        .expect("set the secret");
    let names = vec![
        format!("apps/{}/dev-a1/TOKEN", app.id),
        format!("apps/{}/dev-a1/GONE", app.id),
    ];
    let deleted = delete_named_secrets(&t.db, app.project_id, &names).await;
    assert_eq!(deleted.expect("one stored, one already gone"), 1);
    let again = delete_named_secrets(&t.db, app.project_id, &names).await;
    assert_eq!(again.expect("a second run finds both gone"), 0);
    assert_eq!(secret(&app, "dev-a1").await, None);
    let _ = std::fs::remove_dir_all(tmp);
}
