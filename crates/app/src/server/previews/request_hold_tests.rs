//! The preview-request marker, and the connector and HTTP holds it drives.

use std::sync::{Arc, Mutex};

use agentic_automation::HttpReview;
use agentic_connector::{ConnectorError, DatabaseConnector, ExecutionResult, SqlDialect};
use async_trait::async_trait;

use super::{
    HeldSink, HeldStatement, HoldScope, active, current, hold_if, hold_in, http_review, scope,
    scope_as,
};
use crate::server::previews::hold::Session;
use crate::server::previews::sql_kind::StatementKind;

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
    let held = hold_if(true, recorder.clone(), "warehouse", Session::ReadOnly);
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
    let conn = hold_if(false, recorder.clone(), "warehouse", Session::ReadOnly);
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

/// Hears every held statement, as a staging ask's sink does.
#[derive(Default)]
struct Heard(Mutex<Vec<(String, String)>>);

#[async_trait]
impl HeldSink for Heard {
    async fn held(&self, s: HeldStatement<'_>) {
        let verb = match s.kind {
            StatementKind::Write { verb, .. } => verb.clone(),
            other => format!("{other:?}"),
        };
        self.0.lock().unwrap().push((s.database.to_string(), verb));
    }
}

/// A staging hold's connector tells its sink what it refused, says
/// "staging" rather than "preview", and still sends reads unheard.
#[tokio::test]
async fn a_staging_hold_tells_its_sink_and_names_staging() {
    let heard = Arc::new(Heard::default());
    let app = uuid::Uuid::new_v4();
    let hold = HoldScope::staging(app, heard.clone());
    let recorder = Arc::new(Recorder::default());
    let held = hold_in(
        Some(&hold),
        recorder.clone(),
        "warehouse",
        Session::ReadOnly,
    );

    held.execute_query("SELECT 1", 1)
        .await
        .expect("a read is sent");
    let err = held
        .execute_query("DELETE FROM orders WHERE 1", 1)
        .await
        .expect_err("held")
        .to_string();
    assert!(err.contains("this app's staging environment"), "{err}");
    assert_eq!(
        *heard.0.lock().unwrap(),
        [("warehouse".to_string(), "DELETE".to_string())]
    );
    assert_eq!(*recorder.0.lock().unwrap(), ["SELECT 1"]);
    assert!(
        matches!(hold.http_review("POST"), HttpReview::Hold { reason } if reason.contains("staging"))
    );
    assert!(hold.refusal("Secret `X`").contains("staging environment"));
}

/// The scope carries its hold to everything built inside it; `hold_if` there
/// uses the scope's sink.
#[tokio::test]
async fn the_scope_carries_its_hold() {
    assert!(current().is_none());
    let heard = Arc::new(Heard::default());
    let app = uuid::Uuid::new_v4();
    let recorder = Arc::new(Recorder::default());
    let (app_seen, err) = scope_as(HoldScope::staging(app, heard.clone()), async {
        let held = hold_if(true, recorder.clone(), "warehouse", Session::ReadOnly);
        let err = held.execute_query("DROP TABLE t", 1).await.unwrap_err();
        (current().and_then(|h| h.app_id()), err.to_string())
    })
    .await;
    assert_eq!(app_seen, Some(app));
    assert!(err.contains("staging"), "{err}");
    assert_eq!(heard.0.lock().unwrap().len(), 1);
    assert!(
        recorder.0.lock().unwrap().is_empty(),
        "the DROP was never sent"
    );
    let preview = scope(async { current().map(|h| h.app_id()) }).await;
    assert_eq!(preview, Some(None), "a workspace preview names no app");
}
