//! The Postgres every test in this binary shares.
//!
//! One container, one migration pass, per test *binary* — the statics below are
//! what make that true, so the harness has to live in one module rather than be
//! copied per file (it was, three times, and a fourth copy is what prompted
//! this).
//!
//! `OXY_DATABASE_URL` wins when set; otherwise a reused testcontainer. Reuse
//! hashes the container config, so the tag and shm size must match every other
//! setup site in the workspace or a second container starts silently.

use std::sync::Arc;
use std::time::Duration;

use agentic_airway::extension::AirwayMigrator;
use agentic_runtime::migration::RuntimeMigrator;
use sea_orm::{Database, DatabaseConnection};

static TEST_DB_URL: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();
static TEST_CONTAINER: tokio::sync::OnceCell<
    Arc<testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>>,
> = tokio::sync::OnceCell::const_new();

/// A migrated connection, or `None` when no Postgres is reachable — every
/// caller skips rather than fails, so the suite stays runnable without Docker.
pub async fn test_db() -> Option<DatabaseConnection> {
    let url = TEST_DB_URL
        .get_or_init(|| async {
            if let Ok(url) = std::env::var("OXY_DATABASE_URL") {
                return url;
            }
            use testcontainers::runners::AsyncRunner;
            use testcontainers::{ImageExt, ReuseDirective};
            use testcontainers_modules::postgres::Postgres;
            let container = TEST_CONTAINER
                .get_or_init(|| async {
                    Arc::new(
                        Postgres::default()
                            .with_tag("18-alpine")
                            // 64 MB (Docker default) is too small: a parallel plan wants a 32 MB
                            // DSM segment and a REUSED container accumulates them.
                            // Must match at every setup site — reuse hashes the config.
                            // See internal-docs/workspace-source.md.
                            .with_shm_size(1024 * 1024 * 1024)
                            .with_reuse(ReuseDirective::Always)
                            .start()
                            .await
                            .expect("start Postgres testcontainer — is Docker running?"),
                    )
                })
                .await;
            let port = container.get_host_port_ipv4(5432_u16).await.unwrap();
            format!("postgresql://postgres:postgres@127.0.0.1:{port}/postgres")
        })
        .await
        .clone();

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
