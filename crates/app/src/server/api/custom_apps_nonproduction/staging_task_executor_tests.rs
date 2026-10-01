use std::cell::Cell;

use agentic_core::delegation::TaskSpec;
use uuid::Uuid;

use super::*;
use crate::server::api::custom_apps_nonproduction::staging_task::StagingBuild;

fn task(store: StagingStore) -> StagingMigrationTask {
    let build = StagingBuild {
        app_id: Uuid::from_u128(1),
        app_slug: "shop",
        workspace_id: Uuid::from_u128(2),
        org_id: Uuid::from_u128(3),
        build_pk: Uuid::from_u128(4),
    };
    let declared = [DeclaredMigration {
        filename: "0001_init.sql".into(),
        checksum: "c".into(),
        sql: "CREATE TABLE app_shop.t (id VARCHAR);".into(),
    }];
    StagingMigrationTask::new(store, &build, &declared)
}

#[test]
fn a_task_round_trips_through_its_spec_and_is_keyed_by_app_build_and_store() {
    let airhouse = task(StagingStore::Airhouse);
    let spec = airhouse.spec().expect("encode");
    assert_eq!(
        StagingMigrationTask::from_spec(&spec).expect("decode"),
        airhouse
    );
    assert_eq!(
        airhouse.declared()[0].sql,
        "CREATE TABLE app_shop.t (id VARCHAR);"
    );

    assert_eq!(
        airhouse.run_id(),
        task(StagingStore::Airhouse).run_id(),
        "stable"
    );
    assert_ne!(
        airhouse.run_id(),
        task(StagingStore::OltpBranch).run_id(),
        "per store"
    );
    let mut next_build = task(StagingStore::Airhouse);
    next_build.build_pk = Uuid::from_u128(5);
    assert_ne!(airhouse.run_id(), next_build.run_id(), "per build");

    let other = TaskSpec::Custom {
        kind: "preagg_cycle".into(),
        payload: serde_json::json!({}),
    };
    assert!(StagingMigrationTask::from_spec(&other).is_err());
}

#[tokio::test]
async fn a_busy_lock_is_waited_out_and_anything_else_is_not() {
    let delays = [Duration::ZERO; 3];
    let calls = Cell::new(0);
    let busy_twice = retry_busy(&delays, || {
        calls.set(calls.get() + 1);
        let n = calls.get();
        async move {
            if n <= 2 {
                Err(MigrationError::Busy)
            } else {
                Ok(n)
            }
        }
    })
    .await;
    assert_eq!(busy_twice.expect("third try"), 3);

    calls.set(0);
    let always: Result<(), _> = retry_busy(&delays, || {
        calls.set(calls.get() + 1);
        async { Err(MigrationError::Busy) }
    })
    .await;
    assert!(matches!(always, Err(MigrationError::Busy)));
    assert_eq!(calls.get(), 4, "one try, then one after each delay");

    calls.set(0);
    let failed: Result<(), _> = retry_busy(&delays, || {
        calls.set(calls.get() + 1);
        async { Err(MigrationError::Db("down".into())) }
    })
    .await;
    assert!(matches!(failed, Err(MigrationError::Db(_))));
    assert_eq!(calls.get(), 1, "only Busy is retried");
}

#[tokio::test]
async fn a_stuck_apply_is_dropped_at_its_deadline() {
    let stuck = bounded(Duration::from_millis(20), std::future::pending::<()>()).await;
    assert_eq!(stuck, None);
    assert_eq!(bounded(Duration::from_secs(5), async { 7 }).await, Some(7));
}
