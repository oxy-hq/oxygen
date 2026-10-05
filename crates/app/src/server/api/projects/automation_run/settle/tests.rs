//! `settle` against a real database: the stamp, the first-terminal-wins
//! close, and the poll's reconcile of a run its driver gave up on.

use sea_orm::ActiveModelTrait;

use super::*;

/// A `running` row, in a workspace of its own: the table's
/// `workspace_id` is a real foreign key.
async fn running_row(db: &DatabaseConnection, cancel_requested: bool) -> Uuid {
    let id = Uuid::new_v4();
    let now: chrono::DateTime<chrono::FixedOffset> = Utc::now().into();
    let workspace_id = Uuid::new_v4();
    entity::workspaces::ActiveModel {
        id: ActiveValue::Set(workspace_id),
        name: ActiveValue::Set(format!("settle-{workspace_id}")),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    ProcRunActiveModel {
        id: ActiveValue::Set(id),
        workspace_id: ActiveValue::Set(workspace_id),
        procedure_id: ActiveValue::Set("weekly".into()),
        status: ActiveValue::Set("running".into()),
        params: ActiveValue::Set(None),
        progress_step: ActiveValue::Set(None),
        progress_percent: ActiveValue::Set(None),
        result_summary: ActiveValue::Set(None),
        result_outputs: ActiveValue::Set(None),
        error_message: ActiveValue::Set(None),
        error_code: ActiveValue::Set(None),
        cancel_requested_at: ActiveValue::Set(cancel_requested.then_some(now)),
        started_at: ActiveValue::Set(now),
        execution_started_at: ActiveValue::Set(None),
        execution_heartbeat_at: ActiveValue::Set(None),
        completed_at: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed run");
    id
}

async fn row(db: &DatabaseConnection, id: Uuid) -> proc_run::Model {
    proc_run::Entity::find_by_id(id)
        .one(db)
        .await
        .expect("lookup")
        .expect("row")
}

async fn status(db: &DatabaseConnection, id: Uuid) -> String {
    row(db, id).await.status
}

/// The whole of at-most-once: of any number of attempts at one run, exactly
/// one gets `true` from the stamp, and nothing reopens the run afterwards.
#[tokio::test]
async fn exactly_one_attempt_begins_a_run() {
    let Some(db) = crate::server::test_support::test_db().await else {
        return;
    };
    let id = running_row(&db, false).await;
    assert!(row(&db, id).await.execution_started_at.is_none());

    assert!(begin_execution(&db, id).await.expect("first attempt"));
    let stamped = row(&db, id).await;
    assert!(stamped.execution_started_at.is_some());
    assert_eq!(stamped.status, "running", "beginning is not closing");

    assert!(
        !begin_execution(&db, id).await.expect("second attempt"),
        "a second attempt at a run that already began must be refused"
    );
    assert_eq!(
        row(&db, id).await.execution_started_at,
        stamped.execution_started_at,
        "the refused attempt must not move the stamp"
    );

    // Closing the run leaves the stamp as the record of who ran it.
    assert!(
        close_running(&db, id, interrupted(), Guard::Running)
            .await
            .expect("close")
    );
    let closed = row(&db, id).await;
    assert_eq!(closed.status, "failed");
    assert_eq!(closed.error_code.as_deref(), Some(INTERRUPTED_CODE));
    assert_eq!(closed.error_message.as_deref(), Some(INTERRUPTED_MESSAGE));
    assert!(!begin_execution(&db, id).await.expect("after close"));
}

/// A pending cancel and a closed row are both "not yours to begin".
#[tokio::test]
async fn a_cancelled_or_closed_run_is_never_begun() {
    let Some(db) = crate::server::test_support::test_db().await else {
        return;
    };
    let cancel_stamped = running_row(&db, true).await;
    assert!(
        !begin_execution(&db, cancel_stamped).await.expect("begin"),
        "a cancel stamped before the claim must stop the run before it starts"
    );
    assert!(
        row(&db, cancel_stamped)
            .await
            .execution_started_at
            .is_none()
    );

    let closed = running_row(&db, false).await;
    assert!(
        close_running(&db, closed, cancelled(), Guard::Running)
            .await
            .expect("close")
    );
    assert!(!begin_execution(&db, closed).await.expect("begin"));
    assert!(row(&db, closed).await.execution_started_at.is_none());
}

/// The race the spawned task used to settle by reading the stamp before it
/// wrote: a result that arrives after its user cancelled must not land.
#[tokio::test]
async fn a_result_does_not_land_on_a_run_its_user_cancelled() {
    let Some(db) = crate::server::test_support::test_db().await else {
        return;
    };
    let outputs = HashMap::from([("step".to_string(), serde_json::json!("ok"))]);

    let cancelled_mid_run = running_row(&db, true).await;
    let closed = close_running(
        &db,
        cancelled_mid_run,
        done(&outputs),
        Guard::RunningAndNotCancelled,
    )
    .await
    .expect("close");
    assert!(!closed, "the result must lose to the cancel");
    assert_eq!(status(&db, cancelled_mid_run).await, "running");
    assert!(
        close_running(&db, cancelled_mid_run, cancelled(), Guard::Running)
            .await
            .expect("close")
    );
    assert_eq!(status(&db, cancelled_mid_run).await, "cancelled");

    // The control: the same write lands on a run nobody cancelled.
    let untouched = running_row(&db, false).await;
    assert!(
        close_running(
            &db,
            untouched,
            done(&outputs),
            Guard::RunningAndNotCancelled
        )
        .await
        .expect("close")
    );
    assert_eq!(status(&db, untouched).await, "done");
}

/// First terminal state wins: no later write moves a closed run, so a
/// second attempt after a requeue cannot overwrite the first one's result.
#[tokio::test]
async fn a_closed_run_is_never_reopened_or_rewritten() {
    let Some(db) = crate::server::test_support::test_db().await else {
        return;
    };
    let id = running_row(&db, false).await;
    assert!(
        close_running(&db, id, cancelled(), Guard::Running)
            .await
            .expect("close")
    );
    for late in [
        done(&HashMap::new()),
        failed_with("automation_run_failed", "late".into()),
    ] {
        assert!(
            !close_running(&db, id, late, Guard::Running)
                .await
                .expect("close")
        );
    }
    assert_eq!(status(&db, id).await, "cancelled");
}

// ── The poll's reconcile: closes only what nothing holds ─────────────────────

/// What `cleanup_stale_runs` (`agentic-runtime`,
/// `orchestrator/crud/recovery.rs`) writes at an `oxy serve` boot for a root
/// run with no events that it takes for an orphan. Today that needs an absent
/// or terminal queue entry; a build from before it spared `claimed` entries
/// writes it under a live driver too, and that is the case the first test
/// below keeps covered. Written directly rather than by calling the sweep: the
/// sweep is global and lib tests share one database — and the current sweep no
/// longer produces the claimed case at all.
async fn fail_as_the_boot_sweep_does(db: &DatabaseConnection, run_id: Uuid) {
    use sea_orm::ConnectionTrait;
    db.execute_unprepared(&format!(
        "UPDATE agentic_runs SET task_status = 'failed', \
         error_message = 'server restarted: run never started' WHERE id = '{run_id}'"
    ))
    .await
    .expect("fail the run as the boot sweep does");
}

async fn poll(db: &DatabaseConnection, run_id: Uuid) -> proc_run::Model {
    reconcile_abandoned(db, row(db, run_id).await).await
}

/// The review's sequence: a driver claims the run and is still preparing it
/// (no events yet), a serve pod boots and fails the run under it — a pod on a
/// build from before the boot sweep spared a claimed entry; the current sweep
/// leaves this run alone (`agentic-runtime`'s `stale_run_cleanup_test`). The
/// poll must not close the row — the driver is about to run the steps, and a
/// close here discards its result and sends the user to run them a second
/// time. The same holds once the claim goes back to `queued` (a reaped or
/// released claim): the next claimant settles the row itself.
#[tokio::test]
async fn a_run_failed_under_a_driver_that_holds_it_is_left_running() {
    const WORKER: &str = "settle-test-held";
    let Some(db) = crate::server::test_support::test_db().await else {
        return;
    };
    let id = crate::server::api::projects::automation_run::task::submitted_for_test(&db).await;
    let task_id = id.to_string();

    let claimed = agentic_runtime::crud::claim_task_under_root(&db, WORKER, &task_id)
        .await
        .expect("claim")
        .expect("the run's entry is claimable");
    assert_eq!(claimed.queue_status, "claimed");
    // Claimed first: a `queued` entry has always been spared, so the run the
    // older sweep failed under a live driver was a claimed one.
    fail_as_the_boot_sweep_does(&db, id).await;

    let polled = poll(&db, id).await;
    assert_eq!(
        polled.status, "running",
        "a run whose task is claimed must not be closed by the poll"
    );
    assert_eq!(polled.error_code, None);

    assert!(
        agentic_runtime::crud::release_claim(&db, &task_id, WORKER)
            .await
            .expect("release"),
        "the claim goes back to the queue"
    );
    assert_eq!(
        poll(&db, id).await.status,
        "running",
        "a queued entry still holds the run"
    );
    assert_eq!(status(&db, id).await, "running");
}

/// The control, and the behaviour the reconcile exists for: once the driver
/// has let go of the task, a failed run is closed as orphaned rather than
/// polling `running` until the two-hour sweep.
#[tokio::test]
async fn a_failed_run_nothing_holds_is_closed_as_orphaned() {
    const WORKER: &str = "settle-test-released";
    let Some(db) = crate::server::test_support::test_db().await else {
        return;
    };
    let id = crate::server::api::projects::automation_run::task::submitted_for_test(&db).await;
    let task_id = id.to_string();
    agentic_runtime::crud::claim_task_under_root(&db, WORKER, &task_id)
        .await
        .expect("claim")
        .expect("the run's entry is claimable");
    agentic_runtime::crud::fail_queue_task(&db, &task_id, WORKER)
        .await
        .expect("the driver gives the task up");
    fail_as_the_boot_sweep_does(&db, id).await;

    let polled = poll(&db, id).await;
    assert_eq!(polled.status, "failed");
    assert_eq!(polled.error_code.as_deref(), Some(ORPHANED_CODE));
    assert_eq!(
        polled.error_message.as_deref(),
        Some("automation was interrupted and could not be resumed")
    );
}
