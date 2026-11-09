//! The Postgres every test in this binary shares.
//!
//! One container, one migration pass, per test *binary* — the statics below are
//! what make that true, so the harness has to live in one module rather than be
//! copied per file (it was, three times, and a fourth copy is what prompted
//! this).
//!
//! `OXY_DATABASE_URL` wins when set; otherwise this run's database on the shared
//! test container (`oxy_test_utils::containers::shared_db_url`).

use std::time::Duration;

use agentic_airway::extension::AirwayMigrator;
use agentic_runtime::migration::RuntimeMigrator;
use sea_orm::{Database, DatabaseConnection};

/// A migrated connection, or `None` when no Postgres is reachable — every
/// caller skips rather than fails, so the suite stays runnable without Docker.
pub async fn test_db() -> Option<DatabaseConnection> {
    let url = oxy_test_utils::containers::shared_db_url::<RuntimeMigrator>().await;

    let mut db = None;
    for attempt in 0..10 {
        match Database::connect(&url).await {
            Ok(conn) => {
                db = Some(conn);
                break;
            }
            Err(e) if attempt < 9 => {
                tokio::time::sleep(Duration::from_millis(500)).await;
                eprintln!("test_db: attempt {attempt} failed: {e}, retrying");
            }
            Err(e) => panic!("connect to test DB failed after 10 retries: {e}"),
        }
    }
    let db = db?;
    // Central then runtime (production order — see
    // oxy_test_utils::migration), then AirwayMigrator: airway_run_extensions.run_id
    // FKs to agentic_runs.id, so it must land after runtime.
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
