//! Tests for `reset_task_to_queued` — the in-place task revival that backs the
//! reset-in-place airway retry. It must (a) flip a still-present task back to
//! `queued` and clear its claim bookkeeping, and (b) report 0 rows when the task
//! was reaped, so the caller falls back to a fresh clone-reseed.
//!
//! Run:
//!   cargo nextest run -p agentic-runtime --test integration -E 'test(reset_task_to_queued_test)'

use agentic_core::delegation::TaskSpec;
use agentic_runtime::crud;
use agentic_runtime::migration::RuntimeMigrator;
use sea_orm::{ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};

/// Guards migrations so they run exactly once across the binary's parallel
/// tests — see `test_db`.
static MIGRATED: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

async fn test_db() -> Option<DatabaseConnection> {
    let url = oxy_test_utils::containers::shared_db_url::<RuntimeMigrator>().await;

    let db = Database::connect(&url)
        .await
        .expect("failed to connect to test DB");
    // Migrate exactly once across the binary's parallel tests. Running
    // the migrators per-test races on the shared reused container:
    // concurrent `CREATE TABLE IF NOT EXISTS seaql_migrations_orchestrator`
    // trips Postgres' `pg_type` unique index (a known IF-NOT-EXISTS race).
    // Central then runtime (production order — see oxy_test_utils::migration).
    MIGRATED
        .get_or_init(|| async {
            oxy_test_utils::migration::migrate_shared_test_db::<RuntimeMigrator>(&url, &db)
                .await
                .expect("shared migrations failed")
                .finish()
                .await;
        })
        .await;
    Some(db)
}

/// Read a single scalar column off the task's queue row.
async fn queue_col(db: &DatabaseConnection, task_id: &str, col: &str) -> Option<String> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            &format!("SELECT {col}::text AS v FROM agentic_task_queue WHERE task_id = $1"),
            [task_id.into()],
        ))
        .await
        .unwrap();
    row.map(|r| r.try_get::<String>("", "v").unwrap())
}

async fn seed_airway_task(db: &DatabaseConnection) -> String {
    let run_id = format!("aw-reset-{}", uuid::Uuid::new_v4());
    crud::insert_run(db, &run_id, "Q", None, "airway", None, uuid::Uuid::nil())
        .await
        .unwrap();
    // An airway root enqueues with task_id == run_id. The spec is irrelevant to
    // reset_task_to_queued (it keys on task_id), so a cheap Agent spec is fine.
    crud::enqueue_task(
        db,
        &run_id,
        &run_id,
        None,
        &TaskSpec::Agent {
            agent_id: "a".into(),
            question: "q".into(),
            extra: None,
        },
        None,
        crud::TaskScope::Global,
    )
    .await
    .unwrap();
    run_id
}

/// A present-but-terminal task is revived: `queued`, with claim bookkeeping
/// cleared, and one row reported.
#[tokio::test(flavor = "multi_thread")]
async fn reset_task_to_queued_revives_an_existing_task() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let task_id = seed_airway_task(&db).await;

    // Simulate a failed, previously-claimed task in place (no claim race on
    // the shared test DB).
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_task_queue SET queue_status='failed', worker_id='w', \
             last_heartbeat=now(), claimed_at=now(), claim_count=2 WHERE task_id=$1",
        [task_id.clone().into()],
    ))
    .await
    .unwrap();

    let rows = crud::reset_task_to_queued(&db, &task_id).await.unwrap();
    assert_eq!(rows, 1, "one row should be reset");
    assert_eq!(
        queue_col(&db, &task_id, "queue_status").await.as_deref(),
        Some("queued"),
        "task should be back on the queue"
    );
    assert_eq!(
        queue_col(&db, &task_id, "claim_count").await.as_deref(),
        Some("0"),
        "claim_count should be cleared"
    );
    // `worker_id IS NULL` — SELECT the predicate so a cleared (NULL) worker
    // doesn't trip `try_get::<String>` on a NULL cell.
    assert_eq!(
        queue_col(&db, &task_id, "(worker_id IS NULL)")
            .await
            .as_deref(),
        Some("true"),
        "worker_id should be cleared"
    );
}

/// A reaped task (no row) reports 0 rows so the caller falls back to reseed.
#[tokio::test(flavor = "multi_thread")]
async fn reset_task_to_queued_reports_zero_for_a_reaped_task() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let missing = format!("aw-missing-{}", uuid::Uuid::new_v4());
    let rows = crud::reset_task_to_queued(&db, &missing).await.unwrap();
    assert_eq!(rows, 0, "a reaped task must report 0 rows");
}

/// A live (`claimed`) task is NOT reset in place — the guard reports 0 rows and
/// leaves the claim intact, so a stray retry can't revoke an in-flight worker's
/// claim (a double-drive). The caller falls back to a fresh run instead.
#[tokio::test(flavor = "multi_thread")]
async fn reset_task_to_queued_skips_a_live_claimed_task() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let task_id = seed_airway_task(&db).await;

    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_task_queue SET queue_status='claimed', worker_id='w', \
             last_heartbeat=now(), claimed_at=now(), claim_count=1 WHERE task_id=$1",
        [task_id.clone().into()],
    ))
    .await
    .unwrap();

    let rows = crud::reset_task_to_queued(&db, &task_id).await.unwrap();
    assert_eq!(rows, 0, "a live claimed task must not be reset");
    assert_eq!(
        queue_col(&db, &task_id, "queue_status").await.as_deref(),
        Some("claimed"),
        "the live claim must be left intact"
    );
    assert_eq!(
        queue_col(&db, &task_id, "claim_count").await.as_deref(),
        Some("1"),
        "claim bookkeeping must be untouched"
    );
}

/// A reset-in-place retry must clear the prior attempt's terminal error so the
/// UI shows a clean re-run, not a stale failure.
#[tokio::test(flavor = "multi_thread")]
async fn reset_run_for_retry_clears_terminal_error() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let run_id = format!("aw-rrfr-{}", uuid::Uuid::new_v4());
    crud::insert_run(&db, &run_id, "Q", None, "airway", None, uuid::Uuid::nil())
        .await
        .unwrap();
    // Fail it: sets task_status=failed + error_message.
    crud::update_run_failed(&db, &run_id, "boom").await.unwrap();
    let failed = crud::get_run(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(failed.task_status.as_deref(), Some("failed"));
    assert_eq!(failed.error_message.as_deref(), Some("boom"));

    crud::reset_run_for_retry(&db, &run_id).await.unwrap();
    let reset = crud::get_run(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(
        reset.task_status.as_deref(),
        Some("running"),
        "back to running"
    );
    assert_eq!(reset.error_message, None, "terminal error must be cleared");
}

/// A reset-in-place retry must zero the RECOVERY BUDGET.
///
/// `recover_single_run` retires any run whose `attempt` passes
/// `MAX_RECOVERY_ATTEMPTS`, and an explicit user retry is the escape hatch that
/// design defers to. It is not decorative: the retry path is itself driven by
/// recovery (`mark_task_global` → `find_pending_global_runs` →
/// `recover_single_run`), so a retry SPENDS budget. Without the zeroing, the
/// 5th retry of a run would be dead-lettered before doing any work — and
/// permanently, because `increment_attempt` is the only writer and it only ever
/// goes up.
///
/// Guarded here because the zeroing is one field in an `ActiveModel` whose
/// other fields are `..Default::default()`, which is exactly the shape a future
/// tidy-up drops without noticing.
#[tokio::test(flavor = "multi_thread")]
async fn reset_run_for_retry_zeroes_the_recovery_budget() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let run_id = format!("aw-budget-{}", uuid::Uuid::new_v4());
    crud::insert_run(&db, &run_id, "Q", None, "airway", None, uuid::Uuid::nil())
        .await
        .unwrap();

    // Spend the whole budget the way recovery does.
    for expected in 1..=4 {
        let attempt = crud::increment_attempt(&db, &run_id).await.unwrap();
        assert_eq!(
            attempt, expected,
            "increment_attempt must return the NEW value"
        );
    }
    assert_eq!(
        crud::get_run(&db, &run_id).await.unwrap().unwrap().attempt,
        4,
        "budget should be exhausted before the retry"
    );

    crud::update_run_failed(&db, &run_id, "boom").await.unwrap();
    crud::reset_run_for_retry(&db, &run_id).await.unwrap();

    assert_eq!(
        crud::get_run(&db, &run_id).await.unwrap().unwrap().attempt,
        0,
        "a user retry must hand the run a FULL recovery budget back; leaving \
         `attempt` at 4 makes the next recovery exceed it and dead-letter a run \
         the user just asked to re-run"
    );
}

/// A revival must clear the wait-streak, or a retry can dead-letter instantly.
///
/// `first_deferred_at` measures how long a task has been waiting for something
/// it needs (an airway single-flight lease). If a reset carries a stale streak
/// across, a task that deferred yesterday, ran, failed, and is retried today
/// evaluates its FIRST contention against yesterday's clock and is
/// dead-lettered without waiting at all. `enqueue_task` clears it; this is the
/// third door into the same state and needs the same treatment.
#[tokio::test]
async fn reset_clears_the_wait_streak_and_visibility() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let task_id = seed_airway_task(&db).await;

    // Put the row in the shape a long wait then a failure leaves behind:
    // a streak that began well beyond any ceiling, and a future visibility.
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_task_queue \
         SET queue_status = 'failed', \
             first_deferred_at = now() - interval '2 days', \
             available_at = now() + interval '1 hour' \
         WHERE task_id = $1",
        [task_id.clone().into()],
    ))
    .await
    .unwrap();

    let reset = crud::reset_task_to_queued(&db, &task_id).await.unwrap();
    assert_eq!(reset, 1, "precondition: the reset must have applied");

    // Both assertions read a BOOLEAN expression, not the raw column: `queue_col`
    // unwraps a `String`, so a NULL `first_deferred_at` would panic inside the
    // helper rather than compare equal to `None`. Note `boolean::text` is
    // "true"/"false" — `t` is psql's display abbreviation, not the cast.
    assert_eq!(
        queue_col(&db, &task_id, "(first_deferred_at IS NULL)")
            .await
            .as_deref(),
        Some("true"),
        "a revived task must start a fresh streak, not inherit yesterday's"
    );
    // And it must be visible NOW, not an hour from now.
    //
    // Asserted on the column rather than by claiming it: ~400 tests share
    // this database, and the row is visible to their global selections between
    // the reset and the claim. Testing the property directly
    // removes a race that has nothing to do with what is being verified.
    assert_eq!(
        queue_col(&db, &task_id, "(available_at <= now())")
            .await
            .as_deref(),
        Some("true"),
        "a revived task must be immediately visible, not held by a stale available_at"
    );

    // Retire the row: it is globally claimable until something takes it, and a
    // stray one perturbs the peers that claim or count globally.
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "DELETE FROM agentic_task_queue WHERE task_id = $1",
        [task_id.clone().into()],
    ))
    .await
    .unwrap();
}
