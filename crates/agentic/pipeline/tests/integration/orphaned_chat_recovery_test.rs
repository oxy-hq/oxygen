//! The periodic stranded tick — the worker fleet's pass — takes a chat run a
//! dead `oxy serve` replica was driving, and leaves alone a chat a live peer
//! drives and a chat parked on a person.
//!
//! With no `ide` pod, the one-shot startup pass that used to resume these runs
//! runs nowhere; `recover_stranded_runs` is what every `oxy worker` runs each
//! tick. The selection half is pinned in `agentic-runtime`'s
//! `orphaned_chat_selection_test`; this drives the whole pass.
//!
//! Run:
//!   cargo nextest run -p agentic-pipeline --test integration -E 'test(orphaned_chat_recovery_test)'

use std::sync::Arc;

use agentic_pipeline::platform::{IdentityResolver, PlatformContext};
use agentic_runtime::crud;
use agentic_runtime::state::RuntimeState;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

use crate::automation_recovery_test::{FakePlatform, test_db};

const DEAD_DRIVER: &str = "direct-dead-replica";
const LIVE_DRIVER: &str = "direct-live-replica";

/// A chat root in `status` that has run (one event on its log), last touched
/// ten minutes ago, with no lease yet.
async fn seed_chat(db: &DatabaseConnection, ws: Uuid, status: &str) -> String {
    let run_id = format!("chat-{}", Uuid::new_v4());
    crud::insert_run(db, &run_id, "Q", None, "analytics", None, ws)
        .await
        .unwrap();
    crud::insert_event(
        db,
        &run_id,
        0,
        "step_started",
        &serde_json::json!({ "label": "thinking" }),
        0,
    )
    .await
    .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_runs SET task_status = $2, updated_at = now() - interval '10 minutes' \
         WHERE id = $1",
        [run_id.clone().into(), status.into()],
    ))
    .await
    .unwrap();
    run_id
}

/// The serve replica that drove it was killed: its lease stopped beating.
async fn lapse_lease(db: &DatabaseConnection, run_id: &str) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_runs SET driver_id = $2, \
             driver_heartbeat_at = now() - interval '10 minutes' \
         WHERE id = $1",
        [run_id.into(), DEAD_DRIVER.into()],
    ))
    .await
    .unwrap();
}

async fn event_count(db: &DatabaseConnection, run_id: &str) -> usize {
    crud::get_all_events(db, run_id).await.unwrap().len()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_tick_takes_a_dead_replicas_chat_and_nothing_a_live_process_holds() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();

    let orphan = seed_chat(&db, ws, "running").await;
    lapse_lease(&db, &orphan).await;

    let live = seed_chat(&db, ws, "running").await;
    assert!(
        crud::try_acquire_driver(&db, &live, LIVE_DRIVER)
            .await
            .unwrap()
    );
    let live_events = event_count(&db, &live).await;

    let parked = seed_chat(&db, ws, "awaiting_input").await;

    let base: Arc<dyn PlatformContext> = Arc::new(FakePlatform);
    agentic_pipeline::recovery::recover_stranded_runs(
        db.clone(),
        Arc::new(RuntimeState::new()),
        base,
        Arc::new(IdentityResolver),
        Arc::new(agentic_runtime::router::NoopTaskRouter),
        Some(ws),
        None,
        agentic_pipeline::recovery::DrivePolicy::ALL,
    )
    .await;

    // The orphan was taken: recovery spent a unit of its budget on it and the
    // dead replica's lease is gone (held by recovery, or cleared by the
    // terminal write when a run with no checkpoint is closed as interrupted).
    let row = crud::get_run(&db, &orphan).await.unwrap().expect("run");
    assert_eq!(row.attempt, 1, "recovery took the orphan: {row:?}");
    assert_ne!(
        row.driver_id.as_deref(),
        Some(DEAD_DRIVER),
        "the dead replica's lease was taken over: {row:?}"
    );

    // The live peer's chat: not a write on it.
    let row = crud::get_run(&db, &live).await.unwrap().expect("run");
    assert_eq!(
        (
            row.task_status.as_deref(),
            row.attempt,
            row.driver_id.as_deref()
        ),
        (Some("running"), 0, Some(LIVE_DRIVER)),
        "a run a live peer drives was touched: {row:?}"
    );
    assert_eq!(
        event_count(&db, &live).await,
        live_events,
        "no recovery marker, no deleted tail on a live run"
    );

    // Parked on a person: its answer re-drives it, the tick does not.
    let row = crud::get_run(&db, &parked).await.unwrap().expect("run");
    assert_eq!(
        (row.task_status.as_deref(), row.attempt),
        (Some("awaiting_input"), 0),
        "a parked root was touched: {row:?}"
    );
}
