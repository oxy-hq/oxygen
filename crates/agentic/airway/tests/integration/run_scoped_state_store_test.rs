//! Round-trip test for `AirwayRunScopedStateStore` — the run-scoped cursor store
//! behind mid-window backfill resume. The cursor must persist to
//! `airway_run_extensions.resume_state` and reload from it; an absent
//! resume_state must load as an EMPTY cursor (resume from the window start),
//! never the live pipeline cursor. The schema arg to `save` is ignored (a
//! backfill never writes the live schema).
//!
//! Requires Docker (or `OXY_DATABASE_URL`).

use std::collections::HashMap;
use std::sync::Arc;

use agentic_airway::AirwayRunScopedStateStore;
use agentic_runtime::crud;
use airway::Schema;
use airway::state::{PipelineState, ResourceState, StateStore};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};

use crate::harness::test_db;

/// Seed an `agentic_runs` row + its `airway_run_extensions` row (resume_state NULL).
async fn seed_run_with_extension(db: &DatabaseConnection) -> String {
    let run_id = format!("aw-rss-{}", uuid::Uuid::new_v4());
    crud::insert_run(db, &run_id, "Q", None, "airway", None, uuid::Uuid::nil())
        .await
        .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO airway_run_extensions \
             (run_id, pipeline_name, concurrency, resources) \
         VALUES ($1, 'p', 1, '[]'::jsonb)",
        [run_id.clone().into()],
    ))
    .await
    .unwrap();
    run_id
}

/// An absent resume_state loads as an empty cursor; a saved cursor round-trips
/// through resume_state on the run extension.
#[tokio::test(flavor = "multi_thread")]
async fn resume_state_round_trips_the_cursor() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let db = Arc::new(db);
    let run_id = seed_run_with_extension(&db).await;
    let workspace_id = uuid::Uuid::new_v4();
    let store = AirwayRunScopedStateStore::new(Arc::clone(&db), run_id.clone(), workspace_id, "p");

    // Fresh run: no resume_state yet → empty cursor (start from window start).
    let snap = store.load().await.unwrap();
    assert!(
        snap.state.resource_states.is_empty(),
        "absent resume_state must load an empty cursor"
    );

    // Persist a cursor for the `orders` resource.
    let mut state = PipelineState::default();
    state.schema_version_hash = Some("hash-1".to_string());
    state.resource_states.insert(
        "orders".to_string(),
        ResourceState {
            incremental: None,
            custom: HashMap::from([(
                "__connector_state".to_string(),
                serde_json::json!({ "high_water": "2026-06-15" }),
            )]),
        },
    );
    // Schema arg is ignored by the run-scoped store; any Schema works.
    store.save(&state, &Schema::new("p"), 0).await.unwrap();

    // A fresh store reloads the persisted cursor from resume_state.
    let store2 = AirwayRunScopedStateStore::new(Arc::clone(&db), run_id.clone(), workspace_id, "p");
    let snap2 = store2.load().await.unwrap();
    let orders = snap2
        .state
        .resource_states
        .get("orders")
        .expect("orders cursor must persist");
    assert_eq!(
        orders
            .custom
            .get("__connector_state")
            .and_then(|v| v.get("high_water"))
            .and_then(|v| v.as_str()),
        Some("2026-06-15"),
        "the high-water cursor must round-trip"
    );
    assert_eq!(
        snap2.state.schema_version_hash.as_deref(),
        Some("hash-1"),
        "schema_version_hash must round-trip"
    );
}
