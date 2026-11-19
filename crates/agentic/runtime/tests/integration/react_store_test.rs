//! `DbTranscriptStore` returns a run's rows in seq order.
//!
//! Run:
//!   cargo nextest run -p agentic-runtime --test integration -E 'test(react_store)'

use agentic_runtime::crud;
use agentic_runtime::react::{DbTranscriptStore, TranscriptStore};
use serde_json::json;

use crate::integration_tests::test_db;

#[tokio::test]
async fn db_store_loads_rows_in_seq_order() {
    let Some(db) = test_db().await else {
        return;
    };
    let run_id = format!("test-{}", uuid::Uuid::new_v4());
    crud::insert_run(
        &db,
        &run_id,
        "Q",
        None,
        "analytics",
        None,
        uuid::Uuid::nil(),
    )
    .await
    .unwrap();
    let row = |seq: i64, t: &str, n: i64| (seq, t.to_string(), json!({ "n": n }).to_string(), 0);
    let events = vec![
        row(2, "react_tool_result", 2),
        row(0, "react_turn", 0),
        row(3, "done", 3),
        row(1, "llm_token", 1),
    ];
    crud::batch_insert_events(&db, &run_id, &events)
        .await
        .unwrap();

    let rows = DbTranscriptStore::new(db).load(&run_id).await.unwrap();
    let got: Vec<_> = rows
        .iter()
        .map(|(t, p)| (t.as_str(), p["n"].as_i64().unwrap()))
        .collect();
    assert_eq!(
        got,
        [
            ("react_turn", 0),
            ("llm_token", 1),
            ("react_tool_result", 2),
            ("done", 3)
        ]
    );
}
