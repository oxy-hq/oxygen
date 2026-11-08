//! A graceful shutdown must not reopen runs that already finished.
//!
//! Prod, the 14 days to 2026-11-06: every deploy relabelled ~6,300 finished
//! root runs `failed` with "recovery failed: … no checkpoint". Recovery's
//! drive registered the root in `RuntimeState` and never deregistered it, so
//! `shutdown_all` wrote `shutdown` over the finished row; the next process
//! resumed it, found no checkpoint, and failed it. These tests pin both
//! halves of the fix: the drive drops its registration when it ends, and
//! `shutdown_all` never marks a finished run.
//!
//! Run:
//!   cargo nextest run -p agentic-pipeline --test integration -E 'test(graceful_shutdown_relabel_test)'

use std::sync::Arc;
use std::time::Duration;

use agentic_runtime::crud;
use agentic_runtime::state::RuntimeState;
use serde_json::json;
use tokio::sync::{mpsc, watch};
use uuid::Uuid;

use crate::automation_recovery_test::test_db;
use crate::function_retries_test::{Flaky, fire_function, latency_pass_with_state, settle};

/// The prod shape: a scheduled function driven by the latency worker's
/// recovery pass finishes `done`, and the process then shuts down.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_recovery_finished_is_still_done_after_the_next_shutdown() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let function = Flaky::new(0);
    let run_id = fire_function(&db, ws, None).await;
    let state = Arc::new(RuntimeState::new());

    latency_pass_with_state(&db, ws, &function, state.clone()).await;
    let run = settle(&db, &run_id).await;
    assert_eq!(run.task_status.as_deref(), Some("done"));

    // The drive deregisters right after its coordinator returns, which can
    // trail the terminal write by a moment.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while state.cancel_txs.contains_key(&run_id) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the recovery drive ended but left the root registered"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    assert_eq!(state.shutdown_all(&db).await, 0);
    let after = crud::get_run(&db, &run_id).await.unwrap().expect("run");
    assert_eq!(
        after.task_status.as_deref(),
        Some("done"),
        "shutdown reopened a finished run"
    );
    assert_eq!(function.starts(&run_id), 1);
}

/// Defence in depth: whatever is still registered, `shutdown_all` marks only
/// the unfinished runs, and still signals every pipeline to stop.
#[tokio::test(flavor = "multi_thread")]
async fn shutdown_all_marks_only_unfinished_runs() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let state = Arc::new(RuntimeState::new());
    let mut cancels = Vec::new();
    let mut runs = Vec::new();
    for status in ["running", "done", "failed", "cancelled", "timed_out"] {
        let run_id = format!("shutdown-guard-{status}-{}", Uuid::new_v4());
        crud::insert_run(
            &db,
            &run_id,
            "q",
            None,
            "analytics",
            Some(json!({ "agent_id": "analytics" })),
            Uuid::nil(),
        )
        .await
        .expect("insert run");
        crud::update_task_status(&db, &run_id, status, None)
            .await
            .expect("set status");
        let (answer_tx, _) = mpsc::channel::<String>(1);
        let (cancel_tx, cancel_rx) = watch::channel(false);
        state.register(&run_id, answer_tx, cancel_tx);
        cancels.push(cancel_rx);
        runs.push((run_id, status));
    }

    assert_eq!(
        state.shutdown_all(&db).await,
        1,
        "only the running run is marked"
    );

    for (run_id, status) in &runs {
        let after = crud::get_run(&db, run_id).await.unwrap().expect("run");
        let expected = if *status == "running" {
            "shutdown"
        } else {
            status
        };
        assert_eq!(after.task_status.as_deref(), Some(expected), "{run_id}");
    }
    for rx in &cancels {
        assert!(*rx.borrow(), "every registered pipeline is told to stop");
    }
}
