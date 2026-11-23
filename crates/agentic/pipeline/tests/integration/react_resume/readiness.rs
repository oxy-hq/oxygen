//! A served react start on a real Postgres warehouse whose login role the
//! read-only backstop refuses: with the `react_runtime` flag the only switch,
//! the run falls back to the FSM before any react model call, instead of
//! failing or burning turns on refused queries.

use agentic_connector::{DatabaseConnector, PostgresConnector};
use agentic_runtime::migration::RuntimeMigrator;

use super::*;

/// The shared test server's own login is its superuser, which can write
/// server files: the read-only backstop refuses it, so the run is re-stamped
/// `fsm` and the FSM takes it, with no react engine built and no react row.
/// The FSM's model is an address nothing listens on ([`Host`]), so it never
/// reaches a live model; how its run then ends is not this test's concern.
#[tokio::test(flavor = "multi_thread")]
async fn react_start_on_a_superuser_warehouse_falls_back_to_the_fsm() {
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
    until(&db, &run_id, "the run was never re-stamped fsm", |r| {
        r.metadata
            .as_ref()
            .and_then(|m| m.get("runtime"))
            .and_then(|v| v.as_str())
            == Some("fsm")
    })
    .await;
    // The FSM assembling its pipeline params is what says it took the run.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while host.fsm_params.load(Ordering::SeqCst) == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the fsm never took the run"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    assert_eq!(
        host.engines_built.load(Ordering::SeqCst),
        0,
        "no react engine"
    );
    assert!(host.seen().is_empty(), "no react model call");
    let rows = rows(&db, &run_id).await;
    assert_eq!(count(&rows, "runtime_selected"), 0, "{rows:?}");
    assert_eq!(count(&rows, "react_turn"), 0, "{rows:?}");
}
