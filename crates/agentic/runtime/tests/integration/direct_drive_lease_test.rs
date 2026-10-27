//! `lifecycle::driver_lease` — the lease a direct drive holds.
//!
//! A direct drive (chat, builder, a custom-app ask on a serve replica) used to
//! leave no trace in the database that it was alive, so another process's
//! startup pass resumed it under the live driver. Holding the lease is what
//! now keeps every peer off, and what carries a cancel taken by another
//! replica to the process driving the run.
//!
//! Run:
//!   cargo nextest run -p agentic-runtime --test integration -E 'test(direct_drive_lease_test)'

use std::sync::Arc;

use agentic_runtime::crud;
use agentic_runtime::lifecycle::driver_lease::{
    CANCEL_POLL, await_held_leases_released, hold_direct_drive_lease, hold_direct_drive_lease_as,
};
use agentic_runtime::state::RuntimeState;
use sea_orm::DatabaseConnection;

use crate::stuck_run_sweeper_test::{seed_run, test_db};

async fn resumable(db: &DatabaseConnection, run_id: &str) -> bool {
    crud::get_resumable_root_runs(db, Some(uuid::Uuid::nil()))
        .await
        .unwrap()
        .iter()
        .any(|r| r.id == run_id)
}

/// Process A drives a run directly; process B's startup selection and boot
/// cleanup leave it alone while A holds the lease, and see it again the
/// moment A releases it.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_lease_keeps_a_peers_startup_pass_off_the_run() {
    let Some(db) = test_db().await else {
        return;
    };
    let run_id = seed_run(&db, "analytics").await;
    crud::insert_event(&db, &run_id, 0, "step_start", &serde_json::json!({}), 0)
        .await
        .unwrap();
    assert!(
        resumable(&db, &run_id).await,
        "the control: an undriven run is selected"
    );

    let lease = hold_direct_drive_lease(&db, Arc::new(RuntimeState::new()), &run_id).await;
    assert!(lease.is_held());
    assert!(
        !resumable(&db, &run_id).await,
        "process B's startup pass skips it"
    );
    crud::cleanup_stale_runs(&db).await.unwrap();
    let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(
        run.task_status.as_deref(),
        Some("running"),
        "and its boot cleanup too"
    );

    lease.release().await;
    let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(run.driver_id, None, "release gives the lease back");
    assert!(resumable(&db, &run_id).await);
}

/// A second driver never takes a lease a live one holds.
#[tokio::test(flavor = "multi_thread")]
async fn a_lease_held_elsewhere_is_not_taken() {
    let Some(db) = test_db().await else {
        return;
    };
    let run_id = seed_run(&db, "analytics").await;
    assert!(
        crud::try_acquire_driver(&db, &run_id, "recovery-elsewhere")
            .await
            .unwrap()
    );

    let lease = hold_direct_drive_lease(&db, Arc::new(RuntimeState::new()), &run_id).await;
    assert!(!lease.is_held());
    lease.release().await;
    let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(
        run.driver_id.as_deref(),
        Some("recovery-elsewhere"),
        "releasing a lease it never took does not touch the holder's"
    );
}

/// The start stamps the lease when it inserts the run; the drive continues it
/// under the same id. A fresh id would find the start's stamp live and drive
/// with no lease at all.
#[tokio::test(flavor = "multi_thread")]
async fn a_drive_continues_the_lease_its_start_stamped() {
    let Some(db) = test_db().await else {
        return;
    };
    let run_id = seed_run(&db, "analytics").await;
    assert!(
        crud::try_acquire_driver(&db, &run_id, "direct-start")
            .await
            .unwrap()
    );
    assert!(
        !resumable(&db, &run_id).await,
        "a starting run is not orphaned"
    );

    let fresh = hold_direct_drive_lease(&db, Arc::new(RuntimeState::new()), &run_id).await;
    assert!(!fresh.is_held(), "a fresh id cannot take the start's stamp");
    fresh.release().await;

    let lease = hold_direct_drive_lease_as(
        &db,
        Arc::new(RuntimeState::new()),
        &run_id,
        Some("direct-start".into()),
    )
    .await;
    assert!(lease.is_held(), "the start's id continues its own lease");
    lease.release().await;
    let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(run.driver_id, None, "and gives it back when the drive ends");
}

/// A cancel taken by another replica sets only the durable flag (it may not
/// fail a leased run). The lease's ticker carries it to this process's cancel
/// channel.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_requested_elsewhere_reaches_the_driving_process() {
    let Some(db) = test_db().await else {
        return;
    };
    let run_id = seed_run(&db, "analytics").await;
    let state = Arc::new(RuntimeState::new());
    let (answer_tx, _answer_rx) = tokio::sync::mpsc::channel::<String>(1);
    let (cancel_tx, mut cancel_rx) = tokio::sync::watch::channel(false);
    state.register(&run_id, answer_tx, cancel_tx);
    let lease = hold_direct_drive_lease(&db, state, &run_id).await;
    assert!(lease.is_held());

    assert!(
        !crud::fail_undriven_run(&db, &run_id, "cancelled by user")
            .await
            .unwrap(),
        "another replica's cancel must not fail a run a live driver holds"
    );
    crud::request_cancel(&db, &run_id).await.unwrap();

    let fired = tokio::time::timeout(CANCEL_POLL * 3, async {
        while !*cancel_rx.borrow() {
            cancel_rx.changed().await.unwrap();
        }
    })
    .await;
    assert!(
        fired.is_ok(),
        "the cancel flag never reached the driver's channel"
    );
    lease.release().await;
}

/// A graceful shutdown waits for its cancelled drives to release their own
/// leases, and never releases one for a drive that is still running: that
/// drive's run stays off every peer's startup pass until it stops or its
/// lease lapses.
#[tokio::test(flavor = "multi_thread")]
async fn a_graceful_shutdown_leaves_a_still_running_drives_lease_alone() {
    let Some(db) = test_db().await else {
        return;
    };
    let unwinds = seed_run(&db, "analytics").await;
    let stuck = seed_run(&db, "analytics").await;
    let unwinding = hold_direct_drive_lease(&db, Arc::new(RuntimeState::new()), &unwinds).await;
    let still_running = hold_direct_drive_lease(&db, Arc::new(RuntimeState::new()), &stuck).await;
    for run_id in [&unwinds, &stuck] {
        crud::update_task_status(&db, run_id, "shutdown", None)
            .await
            .unwrap();
    }
    // One drive returns from its coordinator a second into the wait.
    let releaser = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        unwinding.release().await;
    });

    let held = await_held_leases_released(std::time::Duration::from_secs(4)).await;
    releaser.await.unwrap();

    assert_eq!(held, 1, "only the still-running drive's lease remains");
    assert!(
        resumable(&db, &unwinds).await,
        "the stopped drive's run resumes at the next boot"
    );
    assert!(
        !resumable(&db, &stuck).await,
        "a peer must not take a run whose drive is still running"
    );
    still_running.release().await;
}
