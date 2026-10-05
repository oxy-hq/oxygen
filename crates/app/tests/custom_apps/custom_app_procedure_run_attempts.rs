//! An earlier attempt's traces, for `custom_app_procedure_run_queue`: a run
//! some attempt began and left — dead, or still executing — and the dead claim
//! the queue then hands to the next driver. No tests of its own.

use std::sync::Arc;
use std::time::{Duration, Instant};

use agentic_runtime::state::RuntimeState;
use axum::Router;
use oxy_app::agentic_wiring::OxyProjectContext;
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::json;

use crate::custom_app_procedure_run_fixture::{
    Fixture, RUN_DEADLINE, drive_once, driver_platform, poll, queue_status,
};

/// Make the run look like one an earlier attempt began and never finished:
/// stamp `execution_started_at` and the first heartbeat as
/// `settle::begin_execution` does right before the first step — ten minutes
/// ago, with no beat since, which is what a dead attempt leaves. Against the
/// base table — the back-compat `customer_app_procedure_runs` view predates
/// both columns.
pub(crate) async fn mark_begun_by_a_dead_attempt(db: &DatabaseConnection, run_id: &str) {
    db.execute_unprepared(&format!(
        "UPDATE customer_app_automation_runs \
         SET execution_started_at = now() - interval '10 minutes', \
             execution_heartbeat_at = now() - interval '10 minutes' \
         WHERE id = '{run_id}' AND execution_started_at IS NULL"
    ))
    .await
    .expect("stamp the dead attempt's start");
}

/// Make the run look like one whose claim was handed on under a driver that
/// is still executing it: begun ten minutes ago, and beating now.
pub(crate) async fn mark_begun_by_a_live_attempt(db: &DatabaseConnection, run_id: &str) {
    db.execute_unprepared(&format!(
        "UPDATE customer_app_automation_runs \
         SET execution_started_at = now() - interval '10 minutes', \
             execution_heartbeat_at = now() \
         WHERE id = '{run_id}' AND execution_started_at IS NULL"
    ))
    .await
    .expect("stamp the live attempt's start and beat");
}

/// Wait until a claimant has handed the run's task back to the queue unrun
/// (a deferral), and return the entry's status.
pub(crate) async fn wait_until_handed_back(db: &DatabaseConnection, run_id: &str) -> String {
    let deadline = Instant::now() + RUN_DEADLINE;
    while Instant::now() < deadline {
        let entry = agentic_runtime::crud::get_queue_entry(db, run_id)
            .await
            .expect("queue lookup")
            .expect("the run's task");
        if entry.first_deferred_at.is_some() {
            return entry.queue_status;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("no claimant ever handed run {run_id} back to the queue");
}

/// The driver died with its claim and its lease: the queue row's heartbeat
/// and the run's lease are both long stale, and nothing else may touch the run
/// until the reaper hands the claim back. Returns the next attempt's driver
/// platform (and the dir that keeps it alive).
pub(crate) async fn kill_the_driver(
    f: &Fixture,
    app: &Router,
    run_id: &str,
) -> (Arc<OxyProjectContext>, tempfile::TempDir) {
    f.t.db
        .execute_unprepared(&format!(
            "UPDATE agentic_task_queue \
             SET queue_status = 'claimed', worker_id = 'dead-worker@1', claim_count = 1, \
                 claimed_at = now() - interval '10 minutes', \
                 last_heartbeat = now() - interval '10 minutes' \
             WHERE task_id = '{run_id}'"
        ))
        .await
        .expect("simulate a dead claim");
    f.t.db
        .execute_unprepared(&format!(
            "UPDATE agentic_runs \
             SET driver_id = 'recovery-dead', \
                 driver_heartbeat_at = now() - interval '10 minutes' \
             WHERE id = '{run_id}'"
        ))
        .await
        .expect("simulate a stale driver lease");

    // While the dead claim stands, no other driver may take the run.
    let (platform, platform_dir) = driver_platform().await;
    let state = Arc::new(RuntimeState::new());
    assert_eq!(
        drive_once(&f.t.db, &state, &platform, f.workspace_id).await,
        0,
        "a claimed task is not pending, however stale its claim"
    );
    assert_eq!(
        poll(app, f.workspace_id, run_id).await,
        json!({ "status": "running" })
    );

    // The reaper hands the stale claim back.
    let reaped = agentic_runtime::crud::reap_stale_tasks(&f.t.db)
        .await
        .expect("reap");
    assert_eq!(reaped.requeued, 1, "the dead claim is requeued: {reaped:?}");
    assert_eq!(
        queue_status(&f.t.db, run_id).await.as_deref(),
        Some("queued")
    );
    (platform, platform_dir)
}
