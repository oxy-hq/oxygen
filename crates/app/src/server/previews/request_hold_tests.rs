//! The preview-request marker, and the connector and HTTP holds it drives.

use std::sync::{Arc, Mutex};

use agentic_automation::HttpReview;
use agentic_connector::{ConnectorError, DatabaseConnector, ExecutionResult, SqlDialect};
use async_trait::async_trait;

use super::{active, hold_if, http_review, scope};

/// Records every statement that reaches it.
#[derive(Default)]
struct Recorder(Mutex<Vec<String>>);

#[async_trait]
impl DatabaseConnector for Recorder {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::CLICKHOUSE
    }
    async fn execute_query(&self, sql: &str, _: u64) -> Result<ExecutionResult, ConnectorError> {
        self.0.lock().unwrap().push(sql.to_string());
        Ok(ExecutionResult::empty())
    }
}

#[tokio::test]
async fn only_a_task_inside_the_scope_is_a_preview_request() {
    assert!(!active(), "an ordinary request");
    let (inside, spawned) = scope(async {
        let spawned = tokio::spawn(async { active() }).await.unwrap();
        (active(), spawned)
    })
    .await;
    assert!(inside);
    assert!(
        !spawned,
        "a task spawned from the request does not inherit it; what outlives the request \
         must capture the answer when it is built"
    );
    assert!(!active(), "the scope ends with the request");
}

#[tokio::test]
async fn a_held_connector_sends_reads_and_refuses_writes() {
    let recorder = Arc::new(Recorder::default());
    let held = hold_if(true, recorder.clone(), "warehouse");
    held.execute_query("SELECT count() FROM orders", 10)
        .await
        .expect("a read is sent");
    let err = held
        .execute_query("DELETE FROM orders WHERE 1", 10)
        .await
        .expect_err("a write is refused")
        .to_string();
    assert!(
        err.contains("`warehouse` cannot be written in a workspace preview"),
        "{err}"
    );
    assert_eq!(
        *recorder.0.lock().unwrap(),
        ["SELECT count() FROM orders"],
        "the DELETE was never sent"
    );
}

#[tokio::test]
async fn an_unheld_connector_is_the_connector_itself() {
    let recorder = Arc::new(Recorder::default());
    let conn = hold_if(false, recorder.clone(), "warehouse");
    conn.execute_query("DELETE FROM orders WHERE 1", 10)
        .await
        .expect("production sends what it is given");
    assert_eq!(recorder.0.lock().unwrap().len(), 1);
}

#[test]
fn only_get_and_head_are_sent() {
    assert_eq!(http_review("GET"), HttpReview::Proceed);
    assert_eq!(http_review("HEAD"), HttpReview::Proceed);
    for method in ["POST", "PUT", "PATCH", "DELETE"] {
        assert!(
            matches!(http_review(method), HttpReview::Hold { .. }),
            "{method}"
        );
    }
}
