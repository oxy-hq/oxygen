//! `merge_run_metadata` adds keys to a run's metadata and keeps the rest:
//! the queued chat path stamps `runtime: react` on a row its handler wrote.
//!
//! Run:
//!   cargo nextest run -p agentic-runtime --test integration -E 'test(run_metadata_merge)'

use agentic_runtime::crud;
use serde_json::json;

use crate::integration_tests::test_db;

#[tokio::test]
async fn merge_keeps_existing_keys_and_fills_a_null_column() {
    let Some(db) = test_db().await else {
        return;
    };
    let kept = format!("test-{}", uuid::Uuid::new_v4());
    let meta = json!({ "agent_id": "a.agentic.yml", "runtime": "fsm" });
    crud::insert_run(
        &db,
        &kept,
        "Q",
        None,
        "analytics",
        Some(meta),
        uuid::Uuid::nil(),
    )
    .await
    .unwrap();
    let empty = format!("test-{}", uuid::Uuid::new_v4());
    crud::insert_run(&db, &empty, "Q", None, "analytics", None, uuid::Uuid::nil())
        .await
        .unwrap();

    for id in [&kept, &empty] {
        crud::merge_run_metadata(&db, id, json!({ "runtime": "react" }))
            .await
            .unwrap();
    }

    let read = |id: String| {
        let db = db.clone();
        async move { crud::get_run(&db, &id).await.unwrap().unwrap().metadata }
    };
    assert_eq!(
        read(kept).await,
        Some(json!({ "agent_id": "a.agentic.yml", "runtime": "react" }))
    );
    assert_eq!(read(empty).await, Some(json!({ "runtime": "react" })));
}
