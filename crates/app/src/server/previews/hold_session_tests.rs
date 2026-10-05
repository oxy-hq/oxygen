//! On Postgres a `HoldingConnector` sends the read-only `SET` before every
//! statement it forwards, serialised with it; Redshift gets none.

use std::sync::{Arc, Mutex};

use agentic_connector::{ConnectorError, DatabaseConnector, ExecutionResult, SqlDialect};
use agentic_core::result::TypedRowStream;
use async_trait::async_trait;
use uuid::Uuid;

use super::pg::READ_ONLY_SESSION;
use super::{HELD_CODE, HoldingConnector, Session};
use crate::server::previews::request_hold::{HeldSink, HeldStatement, HoldScope};
use crate::server::previews::sql_kind::StatementKind;

/// A Postgres connector that records what reaches it, yielding around each
/// call so concurrent callers get every chance to interleave; `set` decides
/// how the read-only `SET` answers.
struct Pg {
    calls: Mutex<Vec<String>>,
    set: SetAnswer,
}

/// How the fake answers the read-only `SET`.
#[derive(Clone, Copy)]
enum SetAnswer {
    Accepted,
    /// The server refuses the statement (`QueryFailed`).
    Rejected,
    /// The warehouse cannot be reached (`ConnectionError`).
    Unreachable,
}

impl Pg {
    fn new(set: SetAnswer) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            set,
        })
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    async fn record(&self, call: String) {
        tokio::task::yield_now().await;
        self.calls.lock().unwrap().push(call);
        tokio::task::yield_now().await;
    }
}

#[async_trait]
impl DatabaseConnector for Pg {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::Postgres
    }
    async fn execute_query(&self, sql: &str, _: u64) -> Result<ExecutionResult, ConnectorError> {
        self.record(format!("query {sql}")).await;
        Ok(ExecutionResult::empty())
    }
    async fn execute_query_full(&self, sql: &str) -> Result<TypedRowStream, ConnectorError> {
        self.record(format!("full {sql}")).await;
        Ok(TypedRowStream::from_rows(vec![], vec![]))
    }
    async fn execute_query_full_untyped(
        &self,
        sql: &str,
    ) -> Result<TypedRowStream, ConnectorError> {
        self.record(format!("untyped {sql}")).await;
        Ok(TypedRowStream::from_rows(vec![], vec![]))
    }
    async fn execute_statement(&self, sql: &str) -> Result<(), ConnectorError> {
        match self.set {
            SetAnswer::Accepted => {}
            SetAnswer::Rejected => {
                return Err(ConnectorError::query_failed(sql, "not on this engine"));
            }
            SetAnswer::Unreachable => {
                return Err(ConnectorError::ConnectionError("connection refused".into()));
            }
        }
        self.record(format!("statement {sql}")).await;
        Ok(())
    }
}

fn set() -> String {
    format!("statement {READ_ONLY_SESSION}")
}

#[derive(Default)]
struct Notes(Mutex<Vec<String>>);

#[async_trait]
impl HeldSink for Notes {
    async fn held(&self, statement: HeldStatement<'_>) {
        if let StatementKind::Write { verb, .. } = statement.kind {
            self.0.lock().unwrap().push(verb.clone());
        }
    }
}

#[tokio::test]
async fn every_forwarded_postgres_statement_follows_the_set() {
    let inner = Pg::new(SetAnswer::Accepted);
    let conn = HoldingConnector::new(inner.clone(), "pg");
    conn.execute_query("DELETE FROM t", 10)
        .await
        .expect_err("a write is held before the session is touched");
    assert!(inner.calls().is_empty(), "{:?}", inner.calls());

    conn.execute_query("SELECT 1", 10).await.expect("a read");
    conn.execute_query_full("SELECT 2").await.expect("a read");
    conn.execute_query_full_untyped("SELECT 3")
        .await
        .expect("a read");
    conn.execute_statement("SELECT 4").await.expect("a read");
    conn.execute_statement_tagged("SELECT 5", "oxy_run='r'")
        .await
        .expect("a read");
    assert_eq!(
        inner.calls(),
        vec![
            set(),
            "full SELECT 1".to_string(),
            set(),
            "full SELECT 2".to_string(),
            set(),
            "untyped SELECT 3".to_string(),
            set(),
            "full SELECT 4".to_string(),
            set(),
            "full SELECT 5\n/*oxy_run='r'*/".to_string(),
        ],
        "the SET goes first, every time; reads skip the temp-table sampler"
    );
}

/// Two reads at once on one connector (a preview run shares one per
/// database): neither can run between the other's `SET` and its statement.
#[tokio::test]
async fn concurrent_reads_never_interleave_with_anothers_set() {
    let inner = Pg::new(SetAnswer::Accepted);
    let conn = HoldingConnector::new(inner.clone(), "pg");
    let (a, b) = tokio::join!(
        conn.execute_query("SELECT 'a'", 10),
        conn.execute_query("SELECT 'b'", 10)
    );
    a.expect("a read");
    b.expect("a read");
    let calls = inner.calls();
    assert_eq!(calls.len(), 4, "{calls:?}");
    for pair in calls.chunks(2) {
        assert_eq!(pair[0], set(), "{calls:?}");
        assert!(pair[1].starts_with("full SELECT"), "{calls:?}");
    }
}

#[tokio::test]
async fn a_postgres_session_that_cannot_be_made_read_only_sends_nothing() {
    let inner = Pg::new(SetAnswer::Rejected);
    let notes = Arc::new(Notes::default());
    let conn = HoldingConnector::under(
        inner.clone(),
        "pg",
        &HoldScope::staging(Uuid::new_v4(), notes.clone()),
    );
    for _ in 0..2 {
        let err = conn.execute_query("SELECT 1", 10).await.unwrap_err();
        let ConnectorError::QueryFailed(details) = err else {
            panic!("a typed refusal");
        };
        assert_eq!(details.code.as_deref(), Some(HELD_CODE));
        assert!(details.message.contains("read-only"), "{}", details.message);
    }
    assert!(inner.calls().is_empty(), "{:?}", inner.calls());
    assert_eq!(*notes.0.lock().unwrap(), vec!["READ_ONLY_SESSION"; 2]);
}

/// A `SET` that never reached a server (the warehouse unreachable) is not a
/// hold: the connection error surfaces as it is, and nothing is noted.
#[tokio::test]
async fn an_unreachable_postgres_is_a_connection_error_not_a_hold() {
    let inner = Pg::new(SetAnswer::Unreachable);
    let notes = Arc::new(Notes::default());
    let conn = HoldingConnector::under(
        inner.clone(),
        "pg",
        &HoldScope::staging(Uuid::new_v4(), notes.clone()),
    );
    let err = conn.execute_query("SELECT 1", 10).await.unwrap_err();
    assert!(
        matches!(&err, ConnectorError::ConnectionError(m) if m == "connection refused"),
        "the connection error, unchanged: {err:?}"
    );
    assert!(!err.to_string().contains(HELD_CODE), "{err}");
    assert!(inner.calls().is_empty(), "{:?}", inner.calls());
    assert!(notes.0.lock().unwrap().is_empty(), "nothing was held");
}

/// Two holding connectors stacked (a held context's connector wrapped again
/// by a preview platform) still read: the outer `SET` reaches the inner one,
/// which admits it rather than holding it as a write.
#[tokio::test]
async fn stacked_holding_connectors_still_read() {
    let inner = Pg::new(SetAnswer::Accepted);
    let conn = HoldingConnector::new(Arc::new(HoldingConnector::new(inner.clone(), "pg")), "pg");
    conn.execute_query("SELECT 1", 10).await.expect("a read");
    assert_eq!(
        inner.calls(),
        vec![set(), set(), "full SELECT 1".to_string()]
    );
}

/// When the `SET` fails below a stack, the refusal is noted once — by the
/// connector that sent it — and passed up as it is.
#[tokio::test]
async fn a_stacked_set_that_fails_is_noted_once() {
    let inner = Pg::new(SetAnswer::Rejected);
    let notes = Arc::new(Notes::default());
    let hold = HoldScope::staging(Uuid::new_v4(), notes.clone());
    let lower = HoldingConnector::under(inner.clone(), "pg", &hold);
    let conn = HoldingConnector::under(Arc::new(lower), "pg", &hold);
    let err = conn.execute_query("SELECT 1", 10).await.unwrap_err();
    assert!(err.to_string().contains("read-only"), "{err}");
    assert!(inner.calls().is_empty(), "{:?}", inner.calls());
    assert_eq!(*notes.0.lock().unwrap(), vec!["READ_ONLY_SESSION"]);
}

/// Redshift reports the Postgres dialect; told apart by its configured type,
/// it gets no `SET` and the connector's own paths, as before.
#[tokio::test]
async fn redshift_gets_no_session_set() {
    let redshift: oxy::config::model::DatabaseType =
        serde_yaml::from_str("type: redshift\nhost: h\nuser: u\ndatabase: d\n")
            .expect("a redshift database type");
    assert_eq!(Session::for_type(&redshift), Session::ClassifierOnly);

    let inner = Pg::new(SetAnswer::Accepted);
    let conn = HoldingConnector::new(inner.clone(), "rs").with_session(Session::ClassifierOnly);
    conn.execute_query("SELECT 1", 10).await.expect("a read");
    conn.execute_query_full("SELECT 2").await.expect("a read");
    conn.execute_statement("DELETE FROM t")
        .await
        .expect_err("the classifier still holds writes");
    assert_eq!(
        inner.calls(),
        vec!["query SELECT 1".to_string(), "full SELECT 2".to_string()]
    );
}
