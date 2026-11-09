//! Tests for `airway_run_extensions.retry_count` bookkeeping — the counter the
//! reset-in-place retry bumps and the UI shows. `increment_retry_count` must add
//! one on each call and be a no-op for a run with no extension row.
//!
//! Run:
//!   cargo nextest run -p agentic-pipeline --test integration -E 'test(airway_retry_count_test)'

use agentic_airway::extension::run_extension::increment_retry_count;
use agentic_pipeline::AirwayMigrator;
use agentic_runtime::crud;
use agentic_runtime::migration::RuntimeMigrator;
use sea_orm::{ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};

async fn test_db() -> Option<DatabaseConnection> {
    let url = oxy_test_utils::containers::shared_db_url::<RuntimeMigrator>().await;

    let db = Database::connect(&url)
        .await
        .expect("failed to connect to test DB");
    // Central then runtime (production order — see oxy_test_utils::migration).
    oxy_test_utils::migration::migrate_shared_test_db::<RuntimeMigrator>(&url, &db)
        .await
        .expect("shared migrations failed")
        .then::<AirwayMigrator>()
        .await
        .expect("airway migrations failed")
        .finish()
        .await;
    Some(db)
}

/// Seed an `agentic_runs` row + its `airway_run_extensions` row (retry_count 0).
async fn seed_run_with_extension(db: &DatabaseConnection) -> String {
    let run_id = format!("aw-rc-{}", uuid::Uuid::new_v4());
    crud::insert_run(db, &run_id, "Q", None, "airway", None, uuid::Uuid::nil())
        .await
        .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO airway_run_extensions \
             (run_id, pipeline_name, concurrency, resources, retry_count) \
         VALUES ($1, 'p', 1, '[]'::jsonb, 0)",
        [run_id.clone().into()],
    ))
    .await
    .unwrap();
    run_id
}

async fn retry_count(db: &DatabaseConnection, run_id: &str) -> Option<i64> {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT retry_count::bigint AS v FROM airway_run_extensions WHERE run_id = $1",
        [run_id.into()],
    ))
    .await
    .unwrap()
    .map(|r| r.try_get::<i64>("", "v").unwrap())
}

/// Each `increment_retry_count` bumps the counter by one.
#[tokio::test(flavor = "multi_thread")]
async fn increment_retry_count_bumps_the_counter() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let run_id = seed_run_with_extension(&db).await;
    assert_eq!(retry_count(&db, &run_id).await, Some(0));

    increment_retry_count(&db, &run_id).await.unwrap();
    assert_eq!(retry_count(&db, &run_id).await, Some(1));

    increment_retry_count(&db, &run_id).await.unwrap();
    assert_eq!(retry_count(&db, &run_id).await, Some(2));
}

/// A run with no extension row (non-airway, or predating the extension) is a
/// no-op — the best-effort bump must not error.
#[tokio::test(flavor = "multi_thread")]
async fn increment_retry_count_is_a_noop_when_no_extension() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let missing = format!("aw-noext-{}", uuid::Uuid::new_v4());
    // Must not error even though no row exists.
    increment_retry_count(&db, &missing).await.unwrap();
    assert_eq!(retry_count(&db, &missing).await, None);
}
