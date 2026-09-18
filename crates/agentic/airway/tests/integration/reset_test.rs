//! Non-airhouse control flow of the reset-schema helpers
//! ([`agentic_airway::reset`]) — the parts cheaply testable against the oxy DB
//! without a live destination:
//! - `clear_pipeline_state` is idempotent (clearing an absent row is a no-op),
//! - `stored_schema_table_names` yields `[]` when no state row exists — the
//!   empty-tables early-return the executor relies on to skip destination
//!   resolution.
//!
//! Requires Docker (or `OXY_DATABASE_URL`).

use crate::harness::test_db;

/// Clearing the state for a pipeline that has none is a no-op, not an error —
/// the executor calls this unconditionally, including on the
/// never-provisioned path, so it must tolerate an absent row (and repeats).
#[tokio::test(flavor = "multi_thread")]
async fn clear_pipeline_state_is_idempotent_on_absent_row() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let workspace_id = uuid::Uuid::new_v4();
    let name = format!("reset-absent-{}", uuid::Uuid::new_v4());
    // No row was ever written for `name`.
    agentic_airway::reset::clear_pipeline_state(&db, workspace_id, &name)
        .await
        .expect("clearing an absent state row must be a no-op");
    // A second call is still a no-op.
    agentic_airway::reset::clear_pipeline_state(&db, workspace_id, &name)
        .await
        .expect("second clear is still a no-op");
}

/// A pipeline with no state row for this workspace has no stored schema, so a
/// reset has nothing to drop — this drives the executor's empty-tables
/// early-return (clear state, skip destination resolution).
#[tokio::test(flavor = "multi_thread")]
async fn stored_schema_table_names_empty_when_no_row() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let workspace_id = uuid::Uuid::new_v4();
    let name = format!("reset-noschema-{}", uuid::Uuid::new_v4());
    let tables = agentic_airway::reset::stored_schema_table_names(&db, workspace_id, &name)
        .await
        .expect("an absent state row loads a default (empty) snapshot, not an error");
    assert!(
        tables.is_empty(),
        "no state row ⇒ no stored schema ⇒ nothing to drop, got: {tables:?}"
    );
}
