//! The answer's table: a react run stores each query result as it is made,
//! so the `result_table` row is written even when the answer comes from
//! another process after a park, and from memory when storing failed. A
//! `chart` call after a resume reads the stored result the same way.

use std::collections::HashMap;
use std::sync::Mutex;

use agentic_core::react::ToolCallReq;
use agentic_runtime::react::{ArchiveError, ArchiveKey, ArchivedResult, ResultArchive};

use async_trait::async_trait;
use serde_json::{Value, json};

use super::*;

/// Keeps results in memory, shared by every host given it.
#[derive(Default)]
struct Kept(Mutex<HashMap<String, ArchivedResult>>);

#[async_trait]
impl ResultArchive for Kept {
    async fn put(&self, key: &ArchiveKey, result: &ArchivedResult) -> Result<(), ArchiveError> {
        self.0.lock().unwrap().insert(key.path(), result.clone());
        Ok(())
    }
    async fn get(&self, key: &ArchiveKey) -> Result<Option<ArchivedResult>, ArchiveError> {
        Ok(self.0.lock().unwrap().get(&key.path()).cloned())
    }
}

/// Refuses every write, as storage that is down would.
struct Down;

#[async_trait]
impl ResultArchive for Down {
    async fn put(&self, _: &ArchiveKey, _: &ArchivedResult) -> Result<(), ArchiveError> {
        Err(ArchiveError::Storage("down".into()))
    }
    async fn get(&self, _: &ArchiveKey) -> Result<Option<ArchivedResult>, ArchiveError> {
        Err(ArchiveError::Storage("down".into()))
    }
}

fn query() -> Decision {
    let call = ToolCallReq {
        id: "q1".into(),
        name: "query".into(),
        input: json!({ "sql": "SELECT 5 AS n" }),
    };
    Decision::CallTools(turn("", vec![call]))
}

fn host(archive: Arc<dyn ResultArchive>, script: Vec<Decision>) -> Arc<Host> {
    Arc::new(Host {
        archive: Some(archive),
        ..Host::build(true, script)
    })
}

fn table_rows(rows: &[(String, Value)]) -> Vec<&Value> {
    rows.iter()
        .filter(|(t, _)| t == "result_table")
        .map(|(_, p)| p)
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_resumed_answer_tables_a_result_stored_before_the_park() {
    let db = db().await;
    let ws = Uuid::new_v4();
    let kept: Arc<Kept> = Arc::default();
    let run_id = queued_chat(&db, ws).await;
    let starter = host(kept.clone(), vec![query(), ask_user()]);
    latency_pass(&db, ws, &starter).await;
    until(&db, &run_id, "the react run never parked", |r| {
        r.task_status.as_deref() == Some("awaiting_input") && r.driver_id.is_none()
    })
    .await;
    // The upload runs in its own task: wait for it to land, not for a time.
    let stored = async {
        while kept.0.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    };
    tokio::time::timeout(Duration::from_secs(10), stored)
        .await
        .expect("r1 was never stored");

    queue_answer(&db, &run_id, "2024").await.unwrap();
    let reply = Decision::Reply(turn("It is 5 {{result:r1|primary}}.", vec![]));
    let resumer = host(kept, vec![reply]);
    latency_pass(&db, ws, &resumer).await;
    let run = until(&db, &run_id, "the resumed run never ended", ended).await;
    assert_eq!(
        run.task_status.as_deref(),
        Some("done"),
        "{:?}",
        run.error_message
    );

    let rows = rows(&db, &run_id).await;
    let tables = table_rows(&rows);
    assert_eq!(tables.len(), 1, "{rows:?}");
    assert_eq!(tables[0]["result_id"], "r1");
    assert_eq!(tables[0]["rows"], json!([[5.0]]));
    assert_eq!(tables[0]["row_count"], 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_store_still_tables_the_answer_from_memory() {
    let db = db().await;
    let ws = Uuid::new_v4();
    let run_id = queued_chat(&db, ws).await;
    let reply = Decision::Reply(turn("It is 5 {{result:r1}}.", vec![]));
    let only = host(Arc::new(Down), vec![query(), reply]);
    latency_pass(&db, ws, &only).await;
    let run = until(&db, &run_id, "the run never ended", ended).await;
    assert_eq!(
        run.task_status.as_deref(),
        Some("done"),
        "{:?}",
        run.error_message
    );
    let rows = rows(&db, &run_id).await;
    assert_eq!(table_rows(&rows).len(), 1, "{rows:?}");
}

/// The `chart` tool reads the run's one result store: after a park, a fresh
/// host draws `r1` from the stored copy, its points the rows as stored.
#[tokio::test(flavor = "multi_thread")]
async fn a_resumed_run_charts_a_result_stored_before_the_park() {
    let db = db().await;
    let ws = Uuid::new_v4();
    let kept: Arc<Kept> = Arc::default();
    let run_id = queued_chat(&db, ws).await;
    let starter = host(kept.clone(), vec![query(), ask_user()]);
    latency_pass(&db, ws, &starter).await;
    until(&db, &run_id, "the react run never parked", |r| {
        r.task_status.as_deref() == Some("awaiting_input") && r.driver_id.is_none()
    })
    .await;
    let stored = async {
        while kept.0.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    };
    tokio::time::timeout(Duration::from_secs(10), stored)
        .await
        .expect("r1 was never stored");

    queue_answer(&db, &run_id, "2024").await.unwrap();
    let chart = ToolCallReq {
        id: "c1".into(),
        name: "chart".into(),
        input: json!({"result_id": "r1", "kind": "bar", "x": "n", "y": ["n"]}),
    };
    let reply = Decision::Reply(turn("It is 5 {{result:r1|primary}}.", vec![]));
    let resumer = host(
        kept,
        vec![Decision::CallTools(turn("", vec![chart])), reply],
    );
    latency_pass(&db, ws, &resumer).await;
    let run = until(&db, &run_id, "the resumed run never ended", ended).await;
    assert_eq!(
        run.task_status.as_deref(),
        Some("done"),
        "{:?}",
        run.error_message
    );
    let rows = rows(&db, &run_id).await;
    let charts: Vec<&Value> = rows
        .iter()
        .filter(|(t, _)| t == "result_chart")
        .map(|(_, p)| p)
        .collect();
    assert_eq!(charts.len(), 1, "{rows:?}");
    assert_eq!(charts[0]["kind"], "bar");
    assert_eq!(charts[0]["rows"], json!([[5.0, 5.0]]));
}
