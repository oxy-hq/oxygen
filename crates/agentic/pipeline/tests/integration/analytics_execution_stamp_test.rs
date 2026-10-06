//! The execution stamp and heartbeat on `analytics_run_extensions`
//! (`agentic_analytics::extension::execution`), against a real database.
//!
//! They live in the analytics crate; the tests live here because this binary
//! is the one with a Postgres fixture that runs the analytics migrator.
//!
//! Run:
//!   cargo nextest run -p agentic-pipeline --test integration -E 'test(analytics_execution_stamp_test)'

use std::time::Duration;

use agentic_analytics::extension::{
    RunExecution, beat_execution, begin_execution, get_run_execution,
};
use agentic_runtime::crud;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

use crate::automation_recovery_test::test_db;

/// An analytics run as a seed leaves it: `running`, with its extension row.
async fn seeded(db: &DatabaseConnection) -> String {
    let run_id = Uuid::new_v4().to_string();
    crud::insert_run(db, &run_id, "q", None, "analytics", None, Uuid::new_v4())
        .await
        .expect("insert run");
    agentic_analytics::insert_run_meta(db, &run_id, "sales", None)
        .await
        .expect("insert extension");
    run_id
}

async fn execution(db: &DatabaseConnection, run_id: &str) -> RunExecution {
    get_run_execution(db, run_id)
        .await
        .unwrap()
        .expect("extension row")
}

/// The guard this whole change rests on: the stamp is taken once, and an
/// attempt that finds it taken is told so and changes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn the_first_attempt_takes_the_run_and_a_second_cannot() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let run_id = seeded(&db).await;
    assert_eq!(
        execution(&db, &run_id).await.last_alive(),
        None,
        "a seeded run has not begun"
    );

    assert!(begin_execution(&db, &run_id).await.unwrap());
    let first = execution(&db, &run_id).await;
    assert!(first.started_at.is_some());
    assert_eq!(
        first.heartbeat_at, first.started_at,
        "the stamp is the attempt's first heartbeat"
    );

    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        !begin_execution(&db, &run_id).await.unwrap(),
        "a second attempt must not take a run already begun"
    );
    assert_eq!(
        execution(&db, &run_id).await,
        first,
        "a refused attempt writes nothing"
    );
}

/// "Not begun" is checked on the row the statement locks, so a race has one
/// winner — never two attempts each believing the run is theirs.
#[tokio::test(flavor = "multi_thread")]
async fn attempts_racing_for_the_stamp_have_exactly_one_winner() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let run_id = seeded(&db).await;

    let attempts: Vec<_> = (0..8)
        .map(|_| {
            let db = db.clone();
            let run_id = run_id.clone();
            tokio::spawn(async move { begin_execution(&db, &run_id).await.unwrap() })
        })
        .collect();
    let mut winners = 0;
    for attempt in attempts {
        if attempt.await.unwrap() {
            winners += 1;
        }
    }
    assert_eq!(winners, 1);
}

/// A cancel that landed before the attempt started is honoured by the same
/// statement that would have taken the run: nothing starts.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_someone_asked_to_cancel_is_not_begun() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let run_id = seeded(&db).await;
    crud::request_cancel(&db, &run_id).await.unwrap();

    assert!(!begin_execution(&db, &run_id).await.unwrap());
    assert_eq!(execution(&db, &run_id).await.started_at, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_that_has_ended_is_not_begun() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    for terminal in ["done", "failed", "cancelled", "timed_out"] {
        let run_id = seeded(&db).await;
        crud::update_task_status(&db, &run_id, terminal, None)
            .await
            .unwrap();

        assert!(!begin_execution(&db, &run_id).await.unwrap(), "{terminal}");
        assert_eq!(execution(&db, &run_id).await.started_at, None, "{terminal}");
    }
}

/// A run with no extension row (a builder run, or any run that is not
/// analytics) has nowhere to carry a stamp, and is never "taken".
#[tokio::test(flavor = "multi_thread")]
async fn a_run_with_no_extension_row_is_not_begun() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let run_id = Uuid::new_v4().to_string();
    crud::insert_run(&db, &run_id, "q", None, "builder", None, Uuid::new_v4())
        .await
        .unwrap();

    assert!(!begin_execution(&db, &run_id).await.unwrap());
    assert!(!beat_execution(&db, &run_id).await.unwrap());
    assert_eq!(get_run_execution(&db, &run_id).await.unwrap(), None);
}

/// The beat moves the heartbeat and nothing else, for as long as the run is
/// open — a suspended run included, since its driver is still holding it —
/// and stops once the run has ended.
#[tokio::test(flavor = "multi_thread")]
async fn the_beat_follows_the_run_until_it_ends() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let run_id = seeded(&db).await;
    assert!(
        !beat_execution(&db, &run_id).await.unwrap(),
        "a run no attempt has begun has nothing to beat"
    );
    assert_eq!(execution(&db, &run_id).await.last_alive(), None);

    assert!(begin_execution(&db, &run_id).await.unwrap());
    let begun = execution(&db, &run_id).await;

    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(beat_execution(&db, &run_id).await.unwrap());
    let beaten = execution(&db, &run_id).await;
    assert_eq!(beaten.started_at, begun.started_at, "the stamp never moves");
    assert!(beaten.heartbeat_at > begun.heartbeat_at);
    assert_eq!(beaten.last_alive(), beaten.heartbeat_at);

    crud::update_task_status(&db, &run_id, "awaiting_input", None)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        beat_execution(&db, &run_id).await.unwrap(),
        "a suspended run is still open"
    );
    let parked = execution(&db, &run_id).await;
    assert!(parked.heartbeat_at > beaten.heartbeat_at);

    crud::update_run_done(&db, &run_id, "42", None)
        .await
        .unwrap();
    assert!(
        !beat_execution(&db, &run_id).await.unwrap(),
        "a run that has ended has nothing left to prove"
    );
    assert_eq!(execution(&db, &run_id).await, parked);
}

/// The rollback half of the migration: a binary from before it names four
/// columns on insert. That insert must keep working against the new schema,
/// and the row must read back as a run no attempt has begun.
#[tokio::test(flavor = "multi_thread")]
async fn an_insert_from_before_the_migration_still_lands_and_reads_as_unbegun() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let run_id = Uuid::new_v4().to_string();
    crud::insert_run(&db, &run_id, "q", None, "analytics", None, Uuid::new_v4())
        .await
        .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO analytics_run_extensions (run_id, agent_id, spec_hint, thinking_mode) \
         VALUES ($1, 'sales', NULL, NULL)",
        [run_id.clone().into()],
    ))
    .await
    .expect("the pre-migration insert");

    assert_eq!(
        execution(&db, &run_id).await,
        RunExecution {
            started_at: None,
            heartbeat_at: None,
        }
    );
    assert!(begin_execution(&db, &run_id).await.unwrap());
}
