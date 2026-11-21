//! Stop must always fire its in-memory tokens, a heartbeat ticker must survive
//! a failed write but stop the task when its claim is lost, and every owned
//! write must be fenced on the claim it belongs to: `worker_id` is
//! process-stable, so only `claim_epoch` tells a stale execution of a task
//! from this process's later re-claim of the same row.
//!
//! Run:
//!   cargo nextest run -p agentic-runtime --test integration -E 'test(stop_and_heartbeat_test)'

use std::sync::Arc;
use std::time::Duration;

use agentic_core::delegation::{ClaimEpoch, NO_CLAIM, TaskOutcome};
use agentic_core::transport::{CoordinatorTransport, WorkerMessage, WorkerTransport};
use agentic_runtime::crud;
use agentic_runtime::crud::queue::TaskScope;
use agentic_runtime::orchestrator::transport::DurableTransport;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};

use super::eviction_safety_test::{seed_task, test_db};

/// Tick for tickers that must reach the database. Each beat is time-boxed at
/// 60% of the interval, so a 20ms tick gives a real round-trip 12ms, which a
/// loaded machine misses on every beat and the ticker never acts.
const DB_TICK: Duration = Duration::from_millis(1000);

/// A connection whose pool is closed: every statement fails.
async fn closed_db() -> DatabaseConnection {
    let db = test_db().await.expect("test db");
    db.clone().close().await.expect("close pool");
    db
}

/// Seed one queued task and a transport scoped to exactly it (the tests share
/// one table, so an unscoped claim could take another test's row).
async fn seeded() -> (Arc<DurableTransport>, DatabaseConnection, String) {
    let db = test_db().await.expect("test db");
    let (_run, task_id) = seed_task(&db, "test", TaskScope::Global).await;
    let transport = DurableTransport::new_scoped(db.clone(), task_id.clone());
    (transport, db, task_id)
}

/// Claim the task the way a worker does, returning the claim's identity.
async fn claim(transport: &DurableTransport) -> ClaimEpoch {
    let assignment = tokio::time::timeout(Duration::from_secs(10), transport.recv_assignment())
        .await
        .expect("the claim must land")
        .expect("an assignment");
    assert_ne!(assignment.claim_epoch, NO_CLAIM, "a claim has an identity");
    assignment.claim_epoch
}

async fn exec(db: &DatabaseConnection, sql: &str, task_id: &str) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        [task_id.into()],
    ))
    .await
    .unwrap();
}

/// A peer takes the row: still `claimed`, same epoch, different worker.
async fn take_by_peer(db: &DatabaseConnection, task_id: &str) {
    exec(
        db,
        "UPDATE agentic_task_queue SET worker_id = 'someone-else' WHERE task_id = $1",
        task_id,
    )
    .await;
}

/// The reaper hands the row back: `queued`, no owner.
async fn requeue(db: &DatabaseConnection, task_id: &str) {
    exec(
        db,
        "UPDATE agentic_task_queue SET queue_status = 'queued', worker_id = NULL, \
         claimed_at = NULL, last_heartbeat = NULL WHERE task_id = $1",
        task_id,
    )
    .await;
}

/// Lose the claim to a peer, then have this process re-claim the row: a new
/// claim under the identical, process-stable `worker_id`.
async fn lose_then_reclaim(
    transport: &DurableTransport,
    db: &DatabaseConnection,
    task_id: &str,
) -> ClaimEpoch {
    take_by_peer(db, task_id).await;
    requeue(db, task_id).await;
    claim(transport).await
}

async fn queue_status(db: &DatabaseConnection, task_id: &str) -> String {
    crud::get_queue_entry(db, task_id)
        .await
        .unwrap()
        .expect("queue row")
        .queue_status
}

async fn forwarded(transport: &DurableTransport) -> Option<WorkerMessage> {
    tokio::time::timeout(Duration::from_secs(2), transport.recv())
        .await
        .ok()
        .flatten()
}

fn outcome(task_id: &str, outcome: TaskOutcome, claim_epoch: ClaimEpoch) -> WorkerMessage {
    WorkerMessage::Outcome {
        task_id: task_id.to_string(),
        outcome,
        claim_epoch,
    }
}

fn done() -> TaskOutcome {
    TaskOutcome::Done {
        answer: "ok".into(),
        metadata: None,
    }
}

async fn cancelled_within(token: &tokio_util::sync::CancellationToken, why: &str) {
    tokio::time::timeout(Duration::from_secs(30), token.cancelled())
        .await
        .unwrap_or_else(|_| panic!("{why}"));
}

// ── Stop ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn cancel_fires_token_even_when_db_write_fails() {
    let transport = DurableTransport::new(closed_db().await);
    let unclaimed = transport.cancellation_token("t-1", NO_CLAIM);
    let claimed = transport.cancellation_token("t-1", 7);

    let res = transport.cancel("t-1").await;

    assert!(res.is_err(), "the DB write must still surface its error");
    assert!(unclaimed.is_cancelled(), "Stop must reach the running task");
    assert!(claimed.is_cancelled(), "and whichever claim is running it");
}

#[tokio::test]
async fn cancel_subtree_fires_root_and_descendants_even_when_db_write_fails() {
    let transport = DurableTransport::new(closed_db().await);
    let root = transport.cancellation_token("r-1", NO_CLAIM);
    let child = transport.cancellation_token("r-1.1", 3);
    let grandchild = transport.cancellation_token("r-1.1.2", NO_CLAIM);
    let other = transport.cancellation_token("r-10.1", 3);

    let res = transport.cancel_subtree("r-1").await;

    assert!(res.is_err(), "the DB write must still surface its error");
    assert!(root.is_cancelled());
    assert!(child.is_cancelled());
    assert!(grandchild.is_cancelled());
    assert!(!other.is_cancelled(), "a sibling run must not be touched");
}

#[tokio::test]
async fn a_stop_that_beat_the_claim_is_honoured_by_the_claim() {
    let transport = DurableTransport::new(closed_db().await);
    // The task is known (assigned) but nothing has claimed it yet.
    let _ = transport.cancellation_token("t-early", NO_CLAIM);
    let _ = transport.cancel("t-early").await;

    assert!(
        transport.cancellation_token("t-early", 4).is_cancelled(),
        "a Stop whose queue write failed must still stop the claim that follows"
    );
}

// ── Heartbeat ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn heartbeat_ticker_survives_failed_writes() {
    let transport = DurableTransport::new(closed_db().await);
    let task_token = transport.cancellation_token("hb-1", 1);

    let ticker = transport.spawn_heartbeat("hb-1", 1, Duration::from_millis(20));
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert!(
        !ticker.is_cancelled(),
        "one failed beat must not end the ticker"
    );
    assert!(
        !task_token.is_cancelled(),
        "a failed write is not a lost claim"
    );
    ticker.cancel();
}

#[tokio::test]
async fn heartbeat_lost_claim_stops_ticker_and_cancels_task() {
    let (transport, db, task_id) = seeded().await;
    let c1 = claim(&transport).await;
    take_by_peer(&db, &task_id).await;

    let task_token = transport.cancellation_token(&task_id, c1);
    let ticker = transport.spawn_heartbeat(&task_id, c1, DB_TICK);

    cancelled_within(&task_token, "a lost claim must cancel the executing task").await;
    assert!(ticker.is_cancelled(), "and the ticker must stop");
}

/// Interactive roots registered via `Coordinator::register_root` have no queue
/// row, yet their virtual worker still spawns a heartbeat. "No row" is not a
/// lost claim: the ticker must stop quietly and never cancel the task.
#[tokio::test]
async fn heartbeat_with_no_queue_row_does_not_cancel_the_task() {
    let db = test_db().await.expect("test db");
    let task_id = format!("never-enqueued-{}", uuid::Uuid::new_v4());

    let transport = DurableTransport::new(db);
    let task_token = transport.cancellation_token(&task_id, NO_CLAIM);
    let ticker = transport.spawn_heartbeat(&task_id, NO_CLAIM, DB_TICK);

    tokio::time::timeout(Duration::from_secs(5), ticker.cancelled())
        .await
        .expect("the ticker should stop on a row that does not exist");
    assert!(
        !task_token.is_cancelled(),
        "a task with no queue row must not be cancelled by its heartbeat"
    );
}

/// An `AutomationDecision` that completes or fails its run stamps its own queue
/// row inside `execute` (`commit_decision`), before its outcome retires the
/// ticker, and a claim's first beat is immediate. A beat that lands after that
/// stamp found a row that is no longer `claimed` and read it as a lost claim:
/// it cancelled the execution still inside `start_task`, which reported
/// `Cancelled`, and the coordinator turned the finished run `cancelled`. The
/// row still names this claim, so the ticker must stop without a cancel.
#[tokio::test]
async fn a_claims_own_terminal_stamp_stops_its_ticker_without_cancelling_the_task() {
    for status in ["completed", "failed"] {
        let (transport, db, task_id) = seeded().await;
        let c1 = claim(&transport).await;
        // What the decision's commit writes: the status, owner and claim kept.
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE agentic_task_queue SET queue_status = $2 WHERE task_id = $1",
            [task_id.clone().into(), status.into()],
        ))
        .await
        .unwrap();

        let task_token = transport.cancellation_token(&task_id, c1);
        let ticker = transport.spawn_heartbeat(&task_id, c1, DB_TICK);

        tokio::time::timeout(Duration::from_secs(5), ticker.cancelled())
            .await
            .unwrap_or_else(|_| panic!("the ticker should stop on a `{status}` row"));
        assert!(
            !task_token.is_cancelled(),
            "a `{status}` row this claim stamped must not cancel the task"
        );
        assert_eq!(queue_status(&db, &task_id).await, status);
    }
}

/// The control for the case above: a requester's Stop also leaves the owner and
/// claim in place, as `cancelled`, and it must still reach the task.
#[tokio::test]
async fn a_requesters_stop_on_the_row_still_cancels_the_task() {
    let (transport, db, task_id) = seeded().await;
    let c1 = claim(&transport).await;
    crud::cancel_queued_task(&db, &task_id).await.unwrap();

    let task_token = transport.cancellation_token(&task_id, c1);
    let ticker = transport.spawn_heartbeat(&task_id, c1, DB_TICK);

    cancelled_within(
        &task_token,
        "a Stop seen through the queue must cancel the task",
    )
    .await;
    assert!(ticker.is_cancelled(), "and the ticker must stop");
}

/// A terminal row that names a newer claim of this same process is not this
/// claim's own stamp: the older execution is stale and is still stopped.
#[tokio::test]
async fn a_terminal_row_of_a_newer_claim_still_cancels_the_older_execution() {
    let (transport, db, task_id) = seeded().await;
    let c1 = claim(&transport).await;
    let c2 = lose_then_reclaim(&transport, &db, &task_id).await;
    assert!(c2 > c1);
    exec(
        &db,
        "UPDATE agentic_task_queue SET queue_status = 'completed' WHERE task_id = $1",
        &task_id,
    )
    .await;

    let old_token = transport.cancellation_token(&task_id, c1);
    let _old_ticker = transport.spawn_heartbeat(&task_id, c1, DB_TICK);

    cancelled_within(&old_token, "a stale claim must still be stopped").await;
}

/// The reaper requeues a row whose claim was lost, and this same transport
/// re-claims it. The lost claim's cancel must stop *that* execution only: the
/// next claim of the same task_id has its own token and starts live.
#[tokio::test]
async fn heartbeat_lost_claim_leaves_a_live_token_for_the_next_claim() {
    let (transport, db, task_id) = seeded().await;
    let c1 = claim(&transport).await;
    let old_claim = transport.cancellation_token(&task_id, c1);
    let _ticker = transport.spawn_heartbeat(&task_id, c1, DB_TICK);
    take_by_peer(&db, &task_id).await;

    cancelled_within(&old_claim, "a lost claim must cancel the executing task").await;

    requeue(&db, &task_id).await;
    let c2 = claim(&transport).await;
    assert_ne!(c1, c2, "a re-claim is a new claim");
    assert!(
        !transport.cancellation_token(&task_id, c2).is_cancelled(),
        "the next claim of this task must not inherit the lost claim's cancel"
    );
}

/// Gap 3. This process re-claims the row before the old ticker beats again. The
/// old ticker used to read `Beat` (same `worker_id`), keep the old copy
/// running, and then be retired by the new claim. Fenced, it reads `Lost`.
#[tokio::test]
async fn old_ticker_is_fenced_out_by_this_process_reclaim() {
    let (transport, db, task_id) = seeded().await;
    let c1 = claim(&transport).await;
    let old_token = transport.cancellation_token(&task_id, c1);
    let c2 = lose_then_reclaim(&transport, &db, &task_id).await;
    let new_token = transport.cancellation_token(&task_id, c2);

    // Only the OLD claim has a ticker: nothing supersedes it, so it must be the
    // epoch in its predicate that ends it.
    let old_ticker = transport.spawn_heartbeat(&task_id, c1, DB_TICK);

    cancelled_within(&old_token, "the old copy must be stopped, not kept alive").await;
    assert!(old_ticker.is_cancelled());
    assert!(!new_token.is_cancelled(), "the re-claim keeps running");
    assert_eq!(queue_status(&db, &task_id).await, "claimed");
}

/// A newer claim's ticker supersedes the older one and stops the older
/// execution at once, without waiting for its next beat.
#[tokio::test]
async fn a_newer_claims_ticker_stops_the_older_execution() {
    let (transport, db, task_id) = seeded().await;
    let c1 = claim(&transport).await;
    let old_token = transport.cancellation_token(&task_id, c1);
    let old_ticker = transport.spawn_heartbeat(&task_id, c1, Duration::from_secs(3600));

    let c2 = lose_then_reclaim(&transport, &db, &task_id).await;
    let new_ticker = transport.spawn_heartbeat(&task_id, c2, DB_TICK);

    assert!(old_ticker.is_cancelled(), "the older ticker is retired");
    assert!(
        old_token.is_cancelled(),
        "and so is the execution behind it"
    );
    assert!(!new_ticker.is_cancelled());
    assert!(
        !transport.cancellation_token(&task_id, c2).is_cancelled(),
        "the newer claim's own token is untouched"
    );
    new_ticker.cancel();
}

/// `claim_count` is a retry budget — a release refunds it — so it cannot be the
/// fence. `claim_epoch` goes up on every claim regardless.
#[tokio::test]
async fn claim_epoch_survives_a_release_that_refunds_claim_count() {
    let (transport, db, task_id) = seeded().await;
    let c1 = claim(&transport).await;
    let count1 = crud::get_queue_entry(&db, &task_id)
        .await
        .unwrap()
        .unwrap()
        .claim_count;

    assert!(
        crud::release_claim(
            &db,
            &task_id,
            agentic_runtime::transport::process_worker_id(),
            Some(c1)
        )
        .await
        .unwrap()
    );
    let c2 = claim(&transport).await;
    let count2 = crud::get_queue_entry(&db, &task_id)
        .await
        .unwrap()
        .unwrap()
        .claim_count;

    assert_eq!(count1, count2, "the budget was refunded and re-charged");
    assert!(c2 > c1, "the identity must not repeat: {c1} then {c2}");
}

// ── Terminal stamps ──────────────────────────────────────────────────────────

/// The reaper requeued the row (or a peer holds it): the task was cancelled
/// only because its claim was lost, so its `Cancelled` must not reach the
/// coordinator — that would mark the run cancelled while a peer re-runs it.
#[tokio::test]
async fn lost_claim_cancel_is_not_forwarded_as_an_outcome() {
    let (transport, db, task_id) = seeded().await;
    let c1 = claim(&transport).await;
    take_by_peer(&db, &task_id).await;

    transport
        .send(outcome(&task_id, TaskOutcome::Cancelled, c1))
        .await
        .unwrap();

    assert!(
        forwarded(&transport).await.is_none(),
        "a lost-claim cancel must not reach the coordinator"
    );
    assert_eq!(queue_status(&db, &task_id).await, "claimed");
}

/// A Stop reaches this task through the queue when it was issued from another
/// process, or after the reaper took the row: the row goes `cancelled` and the
/// beat reads `Lost`. Nobody else will report that run, so its `Cancelled`
/// must still reach the coordinator.
#[tokio::test]
async fn peer_stop_seen_as_lost_claim_is_still_forwarded() {
    let (transport, db, task_id) = seeded().await;
    let c1 = claim(&transport).await;
    exec(
        &db,
        "UPDATE agentic_task_queue SET queue_status = 'cancelled' WHERE task_id = $1",
        &task_id,
    )
    .await;

    transport
        .send(outcome(&task_id, TaskOutcome::Cancelled, c1))
        .await
        .unwrap();

    match forwarded(&transport).await {
        Some(WorkerMessage::Outcome {
            task_id: id,
            outcome: TaskOutcome::Cancelled,
            ..
        }) => assert_eq!(id, task_id),
        other => panic!("a peer's Stop must reach the coordinator, got {other:?}"),
    }
}

/// The lost execution's `Cancelled` lands after this process re-claimed the
/// row as a new execution. It must not stamp the new execution's row, retire
/// its ticker, or reach the coordinator.
#[tokio::test]
async fn lost_claim_cancel_leaves_this_process_reclaim_alone() {
    let (transport, db, task_id) = seeded().await;
    let c1 = claim(&transport).await;
    let c2 = lose_then_reclaim(&transport, &db, &task_id).await;
    let new_ticker = transport.spawn_heartbeat(&task_id, c2, DB_TICK);

    transport
        .send(outcome(&task_id, TaskOutcome::Cancelled, c1))
        .await
        .unwrap();

    assert!(
        forwarded(&transport).await.is_none(),
        "the lost execution's cancel must not reach the coordinator"
    );
    assert_eq!(queue_status(&db, &task_id).await, "claimed");
    assert!(
        !new_ticker.is_cancelled(),
        "the re-claim's ticker must keep beating"
    );
    new_ticker.cancel();
}

/// Gap 1. A lost execution races to `Done` after this process re-claimed the
/// row. It used to match the re-claim's row (same `worker_id`) and stamp it
/// `completed`, stranding the re-claim's live work.
#[tokio::test]
async fn lost_execution_done_does_not_stamp_the_reclaim() {
    let (transport, db, task_id) = seeded().await;
    let c1 = claim(&transport).await;
    let c2 = lose_then_reclaim(&transport, &db, &task_id).await;
    let new_ticker = transport.spawn_heartbeat(&task_id, c2, DB_TICK);

    transport.send(outcome(&task_id, done(), c1)).await.unwrap();

    assert_eq!(
        queue_status(&db, &task_id).await,
        "claimed",
        "the stale Done must not stamp the re-claim's row"
    );
    assert!(
        forwarded(&transport).await.is_none(),
        "nor settle the task under the execution that now owns it"
    );
    assert!(
        !new_ticker.is_cancelled(),
        "nor retire the re-claim's ticker"
    );
    new_ticker.cancel();
}

/// The re-claim finishes. Its `Done` carries the live epoch, so it stamps its
/// own row and is reported as usual.
#[tokio::test]
async fn reclaim_done_still_stamps_after_a_lost_claim() {
    let (transport, db, task_id) = seeded().await;
    let _c1 = claim(&transport).await;
    let c2 = lose_then_reclaim(&transport, &db, &task_id).await;
    let _new_ticker = transport.spawn_heartbeat(&task_id, c2, DB_TICK);

    transport.send(outcome(&task_id, done(), c2)).await.unwrap();

    assert_eq!(queue_status(&db, &task_id).await, "completed");
    assert!(matches!(
        forwarded(&transport).await,
        Some(WorkerMessage::Outcome {
            outcome: TaskOutcome::Done { .. },
            ..
        })
    ));
}

/// Gap 2 (review finding F4). Lose the claim; the old execution ends `Done`;
/// this process re-claims; the user presses Stop. The re-claim's `Cancelled`
/// must be stamped and forwarded — a leftover task_id-keyed marker from the
/// old execution used to swallow it and leave the run `running`.
#[tokio::test]
async fn stop_after_a_lost_claim_and_reclaim_is_stamped_and_forwarded() {
    let (transport, db, task_id) = seeded().await;
    let c1 = claim(&transport).await;
    take_by_peer(&db, &task_id).await;
    let old_token = transport.cancellation_token(&task_id, c1);
    let _old_ticker = transport.spawn_heartbeat(&task_id, c1, DB_TICK);
    cancelled_within(&old_token, "the lost claim must be noticed").await;

    transport.send(outcome(&task_id, done(), c1)).await.unwrap();
    assert!(
        forwarded(&transport).await.is_none(),
        "row is held by a peer"
    );

    let c2 = lose_then_reclaim(&transport, &db, &task_id).await;
    let new_token = transport.cancellation_token(&task_id, c2);
    let new_ticker = transport.spawn_heartbeat(&task_id, c2, DB_TICK);
    assert!(!new_token.is_cancelled());

    transport.cancel(&task_id).await.unwrap();
    assert!(new_token.is_cancelled(), "Stop reaches the re-claim");
    transport
        .send(outcome(&task_id, TaskOutcome::Cancelled, c2))
        .await
        .unwrap();

    assert_eq!(queue_status(&db, &task_id).await, "cancelled");
    assert!(
        matches!(
            forwarded(&transport).await,
            Some(WorkerMessage::Outcome {
                outcome: TaskOutcome::Cancelled,
                ..
            })
        ),
        "the re-claim's Cancelled must reach the coordinator"
    );
    assert!(new_ticker.is_cancelled(), "and retire its own ticker");
}

/// Crud-level: an `adopt` is a claim too, and fences the previous holder.
#[tokio::test]
async fn adopting_a_queued_row_bumps_the_claim_epoch() {
    let (transport, db, task_id) = seeded().await;
    let c1 = claim(&transport).await;
    requeue(&db, &task_id).await;

    let c2 = crud::adopt_queued_task(&db, &task_id, "driver-a")
        .await
        .unwrap()
        .expect("a queued row can be adopted");

    assert!(c2 > c1);
    assert!(
        !crud::update_queue_heartbeat(&db, &task_id, "driver-a", Some(c1))
            .await
            .unwrap(),
        "the previous claim's heartbeat must match nothing"
    );
    assert!(
        crud::update_queue_heartbeat(&db, &task_id, "driver-a", Some(c2))
            .await
            .unwrap()
    );
}
