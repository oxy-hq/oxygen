//! A teardown that cannot finish its Airhouse step leaves the sandbox
//! `deleting` — also on a worker with **no Airhouse configured at all**
//! (review round 2, n5): the ledger says the sandbox has a sibling there, so
//! "nothing to drop" would remove the row and hand the old tables, and a
//! ledger that says its files are applied, to the next sandbox of that name.

use agentic_core::delegation::TaskOutcome;
use oxy_app::server::api::custom_apps_migrations::read_ledger;
use oxy_app::server::api::custom_apps_sandboxes::{SandboxError, TeardownReason, ops};

use crate::custom_app_functions_fixture::seeded_tenant;
use crate::sandbox_teardown::{AIRHOUSE_APP, sandbox};
use crate::sandbox_teardown_task::{
    furnished_sandbox, migrated_sibling, point_airhouse_nowhere, published, row, run_queued,
    use_scratch_homes,
};

/// With ledger rows and no Airhouse on this worker the run fails and the row
/// stays; a sandbox that never had a sibling is torn down on the same worker.
#[tokio::test]
async fn a_worker_without_airhouse_does_not_finish_a_teardown_whose_sibling_exists() {
    let tmp = use_scratch_homes();
    // This worker has no Airhouse.
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        for var in [
            "AIRHOUSE_BASE_URL",
            "AIRHOUSE_ADMIN_TOKEN",
            "AIRHOUSE_WIRE_HOST",
            "AIRHOUSE_WIRE_PORT",
        ] {
            std::env::remove_var(var);
        }
    }
    let t = seeded_tenant().await;
    let (app, build) = published(&t, AIRHOUSE_APP).await;
    furnished_sandbox(&t, &app, build, "a1").await;
    furnished_sandbox(&t, &app, build, "b2").await;
    // dev-a1 was migrated on a node that has Airhouse: its ledger holds rows.
    let home = migrated_sibling(&t, &app, build, "a1").await;
    let (a1, b2) = (sandbox("a1"), sandbox("b2"));
    let guest = t.guest();
    let delete = |environment| {
        ops::begin_delete(
            &t.db,
            &app,
            environment,
            Some(&guest),
            TeardownReason::Deleted,
        )
    };

    let run_id = delete(&a1).await.expect("delete dev-a1");
    let outcome = run_queued(&t.db, &run_id).await;
    let TaskOutcome::Failed(why) = outcome else {
        panic!("a sibling this worker cannot drop must fail the teardown: {outcome:?}");
    };
    assert!(why.contains("dev-a1 was not torn down (Airhouse:"), "{why}");
    assert!(why.contains(home.schema()), "it names the sibling: {why}");
    assert_eq!(
        row(&t.db, app.id, "dev-a1").await,
        Some(true),
        "the row stays, deleting, for a worker that can finish the job"
    );
    let ledger = read_ledger(&t.db, app.id, "airhouse", home.target()).await;
    assert_eq!(ledger.expect("ledger").len(), 2, "its ledger is untouched");

    // dev-b2 never had a sibling: no ledger rows, nothing to drop, torn down.
    let run_id = delete(&b2).await.expect("delete dev-b2");
    let outcome = run_queued(&t.db, &run_id).await;
    assert!(matches!(outcome, TaskOutcome::Done { .. }), "{outcome:?}");
    assert_eq!(row(&t.db, app.id, "dev-b2").await, None);
    let _ = std::fs::remove_dir_all(tmp);
}

/// A step that fails — here Airhouse, unreachable while the ledger says
/// the sandbox has a sibling — fails the task and leaves the row
/// `deleting`: the name stays taken, and a second delete queues a new run.
#[tokio::test]
async fn a_failing_step_leaves_the_sandbox_deleting() {
    let tmp = use_scratch_homes();
    point_airhouse_nowhere();
    let t = seeded_tenant().await;
    let (app, build) = published(&t, AIRHOUSE_APP).await;
    furnished_sandbox(&t, &app, build, "a1").await;
    // The sandbox's sibling was migrated once, so its ledger holds rows.
    migrated_sibling(&t, &app, build, "a1").await;

    let run_id = ops::begin_delete(
        &t.db,
        &app,
        &sandbox("a1"),
        Some(&t.guest()),
        TeardownReason::Deleted,
    )
    .await
    .expect("delete dev-a1");
    let outcome = run_queued(&t.db, &run_id).await;
    let TaskOutcome::Failed(why) = outcome else {
        panic!("the teardown must fail while Airhouse is unreachable: {outcome:?}");
    };
    assert!(why.contains("dev-a1 was not torn down (Airhouse:"), "{why}");
    assert_eq!(
        row(&t.db, app.id, "dev-a1").await,
        Some(true),
        "the row stays, deleting"
    );
    assert_eq!(
        ops::create(&t.db, &app, &sandbox("a1"), &t.guest()).await,
        Err(SandboxError::Deleting("dev-a1".into()))
    );
    // Its run has ended, failed: deleting it again is the retry, a run of
    // its own.
    let again = ops::begin_delete(
        &t.db,
        &app,
        &sandbox("a1"),
        Some(&t.guest()),
        TeardownReason::Deleted,
    )
    .await
    .expect("delete dev-a1 again");
    assert_ne!(again, run_id);
    let _ = std::fs::remove_dir_all(tmp);
}
