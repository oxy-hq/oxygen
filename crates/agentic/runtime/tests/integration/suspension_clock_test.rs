//! The suspend clock must survive a restart, and must measure the *current*
//! suspension.
//!
//! `Coordinator::from_db` derives a task's `suspended_at` from
//! `agentic_run_suspensions.created_at` rather than restarting it at
//! `Instant::now()`. That only works if the column tracks the suspension the
//! task is in right now — there is one row per run, so a multi-step automation
//! that delegates, resumes, and delegates again would otherwise carry its first
//! step's timestamp forever and time out the fifth step on arrival.
//!
//! Both upsert paths therefore refresh `created_at` on conflict. This pins that,
//! because the failure it prevents is silent: a stale timestamp doesn't error,
//! it just fails a healthy pipeline early.
//!
//! Run:
//!   cargo nextest run -p agentic-runtime --test integration -E 'test(suspension_clock_test)'

use agentic_core::human_input::SuspendedRunData;
use agentic_runtime::crud;
use agentic_runtime::migration::RuntimeMigrator;
use sea_orm::{ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::json;

async fn test_db() -> Option<DatabaseConnection> {
    let url = oxy_test_utils::containers::shared_db_url::<RuntimeMigrator>().await;

    let db = Database::connect(&url)
        .await
        .expect("failed to connect to test database");
    oxy_test_utils::migration::migrate_shared_test_db::<RuntimeMigrator>(&url, &db)
        .await
        .expect("shared migrations failed")
        .finish()
        .await;
    Some(db)
}

fn resume_data(step: &str) -> SuspendedRunData {
    SuspendedRunData {
        from_state: "executing".into(),
        original_input: "q".into(),
        trace_id: "t".into(),
        stage_data: json!({}),
        question: format!("delegating {step}"),
        suggestions: vec![],
    }
}

/// Backdate a run's suspension row, standing in for "this suspension started
/// N seconds ago" without making the test wait.
async fn backdate_suspension(db: &DatabaseConnection, run_id: &str, secs: i64) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_run_suspensions \
         SET created_at = now() - make_interval(secs => $2) WHERE run_id = $1",
        [run_id.into(), (secs as f64).into()],
    ))
    .await
    .expect("backdate");
}

#[tokio::test]
async fn re_suspending_refreshes_the_clock_rather_than_keeping_the_first() {
    let Some(db) = test_db().await else {
        return;
    };
    let run_id = format!("susp-clock-{}", uuid::Uuid::new_v4());
    crud::insert_run(&db, &run_id, "Q", None, "workflow", None, uuid::Uuid::nil())
        .await
        .unwrap();

    // Step 1 delegates, and sits suspended for two hours.
    crud::upsert_suspension(&db, &run_id, "step 1", &[], &resume_data("step 1"))
        .await
        .unwrap();
    backdate_suspension(&db, &run_id, 2 * 60 * 60).await;

    let after_first = crud::get_suspension_with_start(&db, &run_id)
        .await
        .unwrap()
        .expect("suspension row exists")
        .0;
    let aged = (crud::now() - after_first).num_seconds();
    assert!(
        (7150..=7250).contains(&aged),
        "precondition: the first suspension should read ~2h old, got {aged}s"
    );

    // The step completes, the parent resumes, and a later step delegates. One
    // row per run, so this is an UPDATE — and it must move the clock.
    crud::upsert_suspension(&db, &run_id, "step 5", &[], &resume_data("step 5"))
        .await
        .unwrap();

    let after_second = crud::get_suspension_with_start(&db, &run_id)
        .await
        .unwrap()
        .expect("suspension row still exists")
        .0;
    let aged = (crud::now() - after_second).num_seconds();
    assert!(
        aged < 60,
        "re-suspending must restart the clock: the fifth step has just begun, \
         but `created_at` reads {aged}s old. Left stale, `Coordinator::from_db` \
         would hand this task a nearly-expired suspend timeout and fail a \
         healthy pipeline on the next restart."
    );
    assert!(
        after_second > after_first,
        "the refreshed timestamp must be newer than the one it replaced"
    );
}

/// An unparseable checkpoint must not take the clock down with it.
///
/// `SuspendedRunData` derives a plain `Deserialize` with no field defaults, so
/// adding a required field makes every row written by the previous binary
/// unparseable — and recovery onto a new binary is precisely when
/// `Coordinator::from_db` reads these. If the timestamp were lost along with
/// the checkpoint, such a task would be handed a fresh full suspend timeout on
/// every recovery: under a deploy cadence shorter than the ceiling, it would
/// never time out at all. It cannot resume either way, so reaching the ceiling
/// promptly is the whole point.
#[tokio::test]
async fn an_unparseable_checkpoint_still_yields_its_timestamp() {
    let Some(db) = test_db().await else {
        return;
    };
    let run_id = format!("susp-clock-bad-{}", uuid::Uuid::new_v4());
    crud::insert_run(&db, &run_id, "Q", None, "workflow", None, uuid::Uuid::nil())
        .await
        .unwrap();
    crud::upsert_suspension(&db, &run_id, "step 1", &[], &resume_data("step 1"))
        .await
        .unwrap();
    backdate_suspension(&db, &run_id, 90 * 60).await;

    // Stand in for "written by a binary with a different shape of
    // SuspendedRunData" — a required field this version cannot supply.
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_run_suspensions SET resume_data = '{\"unexpected\":true}'::jsonb \
         WHERE run_id = $1",
        [run_id.clone().into()],
    ))
    .await
    .expect("corrupt the checkpoint");

    let (started, data) = crud::get_suspension_with_start(&db, &run_id)
        .await
        .unwrap()
        .expect("the row itself still exists");

    assert!(
        data.is_none(),
        "precondition: this checkpoint must be unparseable for the test to mean anything"
    );
    let aged = (crud::now() - started).num_seconds();
    assert!(
        (5350..=5450).contains(&aged),
        "the suspension timestamp must survive an unparseable checkpoint — got {aged}s, \
         expected ~5400s. Collapsed into one Option, this reads as 'no suspension' and \
         `from_db` hands the task a fresh full timeout on every recovery."
    );
}

/// The delegation hot path (`suspend_with_data_txn`) is a second writer of the
/// same row and must agree — it is the one every suspended step goes through.
#[tokio::test]
async fn the_delegation_write_path_also_refreshes_the_clock() {
    let Some(db) = test_db().await else {
        return;
    };
    let run_id = format!("susp-clock-txn-{}", uuid::Uuid::new_v4());
    crud::insert_run(&db, &run_id, "Q", None, "workflow", None, uuid::Uuid::nil())
        .await
        .unwrap();

    crud::suspend_with_data_txn(
        &db,
        &run_id,
        "delegating",
        None,
        "step 1",
        &[],
        &resume_data("step 1"),
    )
    .await
    .unwrap();
    backdate_suspension(&db, &run_id, 3 * 60 * 60).await;

    crud::suspend_with_data_txn(
        &db,
        &run_id,
        "delegating",
        None,
        "step 2",
        &[],
        &resume_data("step 2"),
    )
    .await
    .unwrap();

    let started = crud::get_suspension_with_start(&db, &run_id)
        .await
        .unwrap()
        .expect("suspension row exists")
        .0;
    let aged = (crud::now() - started).num_seconds();
    assert!(
        aged < 60,
        "suspend_with_data_txn must refresh `created_at` too; got {aged}s old"
    );
}
