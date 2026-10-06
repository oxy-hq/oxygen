//! `adopt_queued_task` / `DurableTransport::adopt_queued_claim`: a driver
//! holding a parked task's `queued` row as its own claim, without running it.
//!
//! What the tests pin: only a `queued` row is taken; taking it charges no
//! retry budget and ignores a deferral's delay; and the row then behaves like
//! any other claim — this process's heartbeat matches it, and no worker is
//! handed it.
//!
//! Run:
//!   cargo nextest run -p agentic-runtime --test integration -E 'test(adopt_queued_task_test)'

use agentic_core::delegation::TaskSpec;
use agentic_runtime::crud;
use agentic_runtime::transport::{DurableTransport, process_worker_id};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

use crate::integration_tests::test_db;

fn spec() -> TaskSpec {
    TaskSpec::Agent {
        agent_id: "sales".into(),
        question: "how did the stores do?".into(),
        extra: None,
    }
}

/// A Global root as a reaped claim leaves it: `queued`, start spec, one claim
/// already charged.
async fn requeued_root(db: &DatabaseConnection) -> String {
    let run_id = Uuid::new_v4().to_string();
    crud::insert_run(db, &run_id, "q", None, "analytics", None, Uuid::new_v4())
        .await
        .expect("insert run");
    crud::enqueue_task(
        db,
        &run_id,
        &run_id,
        None,
        &spec(),
        None,
        crud::TaskScope::Global,
    )
    .await
    .expect("enqueue");
    set(db, &run_id, "claim_count = 1").await;
    run_id
}

async fn set(db: &DatabaseConnection, task_id: &str, assignment: &str) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!("UPDATE agentic_task_queue SET {assignment} WHERE task_id = $1"),
        [task_id.into()],
    ))
    .await
    .expect("update queue row");
}

async fn entry(
    db: &DatabaseConnection,
    task_id: &str,
) -> agentic_runtime::entity::task_queue::Model {
    crud::get_queue_entry(db, task_id)
        .await
        .unwrap()
        .expect("queue entry")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_queued_row_is_held_without_spending_retry_budget() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let task_id = requeued_root(&db).await;

    assert!(
        crud::adopt_queued_task(&db, &task_id, "driver-a")
            .await
            .unwrap()
    );

    let row = entry(&db, &task_id).await;
    assert_eq!(row.queue_status, "claimed");
    assert_eq!(row.worker_id.as_deref(), Some("driver-a"));
    assert!(row.last_heartbeat.is_some(), "a claim starts with a beat");
    assert_eq!(
        row.claim_count, 1,
        "nothing ran, so the claim is not charged"
    );
    assert!(
        crud::update_queue_heartbeat(&db, &task_id, "driver-a")
            .await
            .unwrap(),
        "the holder's heartbeat matches the row"
    );
}

/// A row someone holds, and a row that has ended, are not this driver's to
/// take: adopting must never move a peer's live claim or revive a dead task.
#[tokio::test(flavor = "multi_thread")]
async fn only_a_queued_row_is_taken() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    for status in ["claimed", "completed", "failed", "cancelled", "dead"] {
        let task_id = requeued_root(&db).await;
        set(
            &db,
            &task_id,
            &format!("queue_status = '{status}', worker_id = 'peer'"),
        )
        .await;

        assert!(
            !crud::adopt_queued_task(&db, &task_id, "driver-a")
                .await
                .unwrap(),
            "{status}"
        );
        let row = entry(&db, &task_id).await;
        assert_eq!(row.queue_status, status);
        assert_eq!(row.worker_id.as_deref(), Some("peer"), "{status}");
    }
    assert!(
        !crud::adopt_queued_task(&db, "no-such-task", "driver-a")
            .await
            .unwrap()
    );
}

/// A deferral hides a row from workers until its delay passes. Holding a
/// parked run's row is not handing it out to run, so the delay does not apply.
#[tokio::test(flavor = "multi_thread")]
async fn a_deferred_row_is_held_before_its_delay_passes() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let task_id = requeued_root(&db).await;
    set(&db, &task_id, "available_at = now() + interval '1 hour'").await;
    assert!(
        crud::claim_task_under_root(&db, "worker-b", &task_id)
            .await
            .unwrap()
            .is_none(),
        "a worker cannot claim it yet"
    );

    assert!(
        crud::adopt_queued_task(&db, &task_id, "driver-a")
            .await
            .unwrap()
    );
}

/// The transport pairs the claim with the ordinary heartbeat and names this
/// process as the holder, so a worker scoped to the same root finds nothing.
#[tokio::test(flavor = "multi_thread")]
async fn the_transport_holds_the_row_as_this_process_and_no_worker_is_handed_it() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let task_id = requeued_root(&db).await;
    let transport = DurableTransport::new_scoped(db.clone(), task_id.clone());

    assert!(transport.adopt_queued_claim(&task_id).await.unwrap());

    let row = entry(&db, &task_id).await;
    assert_eq!(row.queue_status, "claimed");
    assert_eq!(row.worker_id.as_deref(), Some(process_worker_id()));
    assert!(
        crud::claim_task_under_root(&db, process_worker_id(), &task_id)
            .await
            .unwrap()
            .is_none(),
        "the run's own worker must not be handed the held row"
    );
    assert!(
        !transport.adopt_queued_claim(&task_id).await.unwrap(),
        "already held: a second adoption takes nothing"
    );
}
