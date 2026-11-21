//! A served react start on a real Postgres warehouse whose login role the
//! read-only backstop refuses: the run fails at its start, with the reason,
//! before any model call, instead of burning turns on refused queries.

use agentic_connector::{DatabaseConnector, PostgresConnector};
use agentic_runtime::migration::RuntimeMigrator;

use super::*;

/// The shared test server's own login is its superuser, which can write
/// server files: the backstop refuses it at the build's readiness check.
#[tokio::test(flavor = "multi_thread")]
async fn react_start_on_a_superuser_warehouse_is_refused_before_any_model_call() {
    let db = db().await;
    let url = oxy_test_utils::containers::shared_db_url::<RuntimeMigrator>().await;
    let warehouse = PostgresConnector::from_dsn(&url, false).expect("the test server's DSN");
    let host = Arc::new(Host {
        connector: Some(Arc::new(warehouse) as Arc<dyn DatabaseConnector>),
        ..Host::build(true, vec![])
    });
    let ws = Uuid::new_v4();
    let run_id = queued_chat(&db, ws).await;
    latency_pass(&db, ws, &host).await;
    let run = until(&db, &run_id, "the refused run never ended", ended).await;

    assert_eq!(run.task_status.as_deref(), Some("failed"));
    let message = run.error_message.unwrap_or_default();
    assert!(
        message.contains("runtime: react cannot use warehouse (PostgreSQL)")
            && message.contains("least-privilege role"),
        "{message}"
    );
    assert_eq!(host.engines_built.load(Ordering::SeqCst), 0, "no model");
    assert!(host.seen().is_empty(), "no model call");
    let rows = rows(&db, &run_id).await;
    assert_eq!(count(&rows, "error"), 1, "{rows:?}");
    assert_eq!(count(&rows, "done"), 0, "{rows:?}");
    assert_eq!(count(&rows, "react_turn"), 0, "{rows:?}");
}
