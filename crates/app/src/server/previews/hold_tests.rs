//! `HoldingConnector` never forwards a write, by any method.

use std::sync::{Arc, Mutex};

use agentic_connector::{
    ArrowQueryStream, AsArrowConnector, ConnectorError, DatabaseConnector, ExecutionResult,
    SqlDialect, SqlTransaction,
};
use agentic_core::result::TypedRowStream;
use async_trait::async_trait;

use super::HoldingConnector;

/// A connector that records every call that reaches it and pretends to
/// succeed where it cheaply can.
struct Recorder {
    dialect: SqlDialect,
    calls: Mutex<Vec<String>>,
}

impl Recorder {
    fn new(dialect: SqlDialect) -> Arc<Self> {
        Arc::new(Self {
            dialect,
            calls: Mutex::new(Vec::new()),
        })
    }
    fn record(&self, call: &str) {
        self.calls.lock().unwrap().push(call.to_string());
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

fn recorded() -> ConnectorError {
    ConnectorError::Other("recorded".into())
}

#[async_trait]
impl DatabaseConnector for Recorder {
    fn dialect(&self) -> SqlDialect {
        self.dialect
    }
    async fn execute_query(&self, sql: &str, _: u64) -> Result<ExecutionResult, ConnectorError> {
        self.record(&format!("execute_query {sql}"));
        Ok(ExecutionResult::empty())
    }
    async fn execute_query_full(&self, sql: &str) -> Result<TypedRowStream, ConnectorError> {
        self.record(&format!("execute_query_full {sql}"));
        Err(recorded())
    }
    async fn execute_query_full_untyped(
        &self,
        sql: &str,
    ) -> Result<TypedRowStream, ConnectorError> {
        self.record(&format!("execute_query_full_untyped {sql}"));
        Err(recorded())
    }
    async fn begin_transaction(&self) -> Result<Box<dyn SqlTransaction>, ConnectorError> {
        self.record("begin_transaction");
        Err(recorded())
    }
    fn as_arrow(&self) -> Option<&dyn AsArrowConnector> {
        Some(self)
    }
    async fn execute_statement(&self, sql: &str) -> Result<(), ConnectorError> {
        self.record(&format!("execute_statement {sql}"));
        Ok(())
    }
    async fn execute_statement_tagged(&self, sql: &str, _: &str) -> Result<(), ConnectorError> {
        self.record(&format!("execute_statement_tagged {sql}"));
        Ok(())
    }
    async fn prepare_schema(&self) -> Result<(), ConnectorError> {
        self.record("prepare_schema");
        Ok(())
    }
}

#[async_trait]
impl AsArrowConnector for Recorder {
    async fn execute_query_arrow(&self, sql: &str) -> Result<ArrowQueryStream, ConnectorError> {
        self.record(&format!("execute_query_arrow {sql}"));
        Err(recorded())
    }
}

/// Every method that takes SQL, called once each. `Ok` is "reached the inner
/// connector" (the recorder's own `Err("recorded")` counts as reaching it).
async fn call_every_sql_method(conn: &HoldingConnector, sql: &str) -> Vec<Result<(), String>> {
    let reached = |e: ConnectorError| {
        let message = e.to_string();
        if message.contains("recorded") {
            Ok(())
        } else {
            Err(message)
        }
    };
    vec![
        conn.execute_query(sql, 10)
            .await
            .map(|_| ())
            .or_else(reached),
        conn.execute_query_full(sql)
            .await
            .map(|_| ())
            .or_else(reached),
        conn.execute_query_full_untyped(sql)
            .await
            .map(|_| ())
            .or_else(reached),
        conn.execute_statement(sql).await.or_else(reached),
        conn.execute_statement_tagged(sql, "oxy_run='r'")
            .await
            .or_else(reached),
    ]
}

#[tokio::test]
async fn holding_connector_never_forwards_a_write() {
    for (dialect, sql) in [
        (SqlDialect::Postgres, "DELETE FROM orders WHERE id = 1"),
        (
            SqlDialect::CLICKHOUSE,
            "ALTER TABLE toast.orders DELETE WHERE 1",
        ),
        (
            SqlDialect::CLICKHOUSE,
            "INSERT INTO rollups.daily SELECT * FROM toast.orders",
        ),
        (
            SqlDialect::Postgres,
            "SELECT * INTO scratch.copy FROM orders",
        ),
        (SqlDialect::Postgres, "SELECT 1; DROP TABLE orders"),
        (
            SqlDialect::Snowflake,
            "CREATE OR REPLACE TABLE a AS SELECT 1",
        ),
        (
            SqlDialect::DuckDb,
            "INSERT INTO toast_pos.sales_daily_metrics VALUES (1)",
        ),
        (SqlDialect::Postgres, "SELEC this does not parse"),
        (SqlDialect::Postgres, ""),
    ] {
        let inner = Recorder::new(dialect);
        let conn = HoldingConnector::new(inner.clone(), "warehouse");
        for (i, outcome) in call_every_sql_method(&conn, sql)
            .await
            .into_iter()
            .enumerate()
        {
            let err = outcome.expect_err(&format!("method #{i} forwarded `{sql}`"));
            assert!(err.contains("held") && err.contains("`warehouse`"), "{err}");
        }
        assert!(
            inner.calls().is_empty(),
            "`{sql}` reached the inner connector: {:?}",
            inner.calls()
        );
    }

    // The control: a read goes through every method, so the refusals above
    // are about the SQL and not a wrapper that refuses everything.
    let inner = Recorder::new(SqlDialect::CLICKHOUSE);
    let conn = HoldingConnector::new(inner.clone(), "clickhouse");
    let outcomes = call_every_sql_method(&conn, "SELECT * FROM toast.orders FINAL").await;
    assert!(outcomes.iter().all(Result::is_ok), "{outcomes:?}");
    assert_eq!(inner.calls().len(), 5, "{:?}", inner.calls());
}

#[tokio::test]
async fn a_refusal_names_what_was_held() {
    let conn = HoldingConnector::new(Recorder::new(SqlDialect::Postgres), "warehouse");
    let err = conn
        .execute_statement("UPDATE public.orders SET voided = true")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("`UPDATE` on public.orders"), "{err}");
}

/// A refusal is typed, so an HTTP surface answers it as the preview's `409`
/// without reading the message.
#[tokio::test]
async fn a_refusal_carries_the_preview_code() {
    let conn = HoldingConnector::new(Recorder::new(SqlDialect::Postgres), "warehouse");
    let err = conn
        .execute_query("DELETE FROM orders", 1)
        .await
        .unwrap_err();
    let details = match err {
        ConnectorError::QueryFailed(details) => details,
        other => panic!("a refusal is a typed query failure: {other}"),
    };
    assert_eq!(details.code.as_deref(), Some(super::HELD_CODE));
    assert_eq!(details.sql, "DELETE FROM orders");
}

#[tokio::test]
async fn prepare_schema_is_a_noop() {
    let inner = Recorder::new(SqlDialect::Postgres);
    let conn = HoldingConnector::new(inner.clone(), "warehouse");
    conn.prepare_schema().await.expect("a no-op succeeds");
    assert!(inner.calls().is_empty(), "{:?}", inner.calls());
}

#[tokio::test]
async fn transactions_are_refused() {
    let inner = Recorder::new(SqlDialect::Postgres);
    let conn = HoldingConnector::new(inner.clone(), "warehouse");
    let Err(err) = conn.begin_transaction().await else {
        panic!("a transaction must be refused");
    };
    assert!(err.to_string().contains("held"), "{err}");
    assert!(inner.calls().is_empty(), "{:?}", inner.calls());
}

#[test]
fn arrow_escape_hatch_is_closed() {
    let inner = Recorder::new(SqlDialect::DuckDb);
    assert!(inner.as_arrow().is_some(), "control: the inner one has it");
    let conn = HoldingConnector::new(inner, "airhouse");
    assert!(conn.as_arrow().is_none());
}

/// A source scan: every method of `DatabaseConnector` is stated by the
/// wrapper, and every one that takes `sql: &str` admits it first. A method
/// added to the trait with a default would otherwise be answered by that
/// default — the `BoxedSourceConnector` trap — and could reach the inner
/// connector unguarded.
#[test]
fn every_sql_accepting_method_is_guarded() {
    let trait_src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../agentic/connector/src/connector.rs"
    ))
    .expect("agentic-connector source is readable from the workspace");
    let trait_body = block_after(&trait_src, "pub trait DatabaseConnector");
    let impl_body = block_after(
        include_str!("hold.rs"),
        "impl DatabaseConnector for HoldingConnector",
    );

    let trait_fns = functions(&trait_body);
    let impl_fns = functions(&impl_body);
    assert!(
        trait_fns.len() >= 9,
        "scan found too few trait methods: {trait_fns:?}"
    );
    for (name, signature) in &trait_fns {
        let Some((_, body)) = impl_fns.iter().find(|(n, _)| n == name) else {
            panic!("HoldingConnector does not override `{name}`; its default would answer");
        };
        if signature.contains("sql: &str") {
            assert!(
                body.contains("self.admit(sql)?"),
                "`{name}` takes SQL but does not admit it first"
            );
        }
    }
}

/// The text inside the braces that follow `marker`, with `//` comment lines
/// removed first so a brace in a doc comment cannot unbalance the count.
fn block_after(src: &str, marker: &str) -> String {
    let start = src
        .find(marker)
        .unwrap_or_else(|| panic!("`{marker}` not found"));
    let code: String = src[start..]
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let open = code.find('{').expect("an opening brace");
    let mut depth = 0usize;
    for (i, c) in code[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return code[open + 1..open + i].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced braces after `{marker}`");
}

/// `(name, text from this `fn` to the next)` for every `fn` in a block.
fn functions(block: &str) -> Vec<(String, String)> {
    let starts: Vec<usize> = block.match_indices("fn ").map(|(i, _)| i).collect();
    starts
        .iter()
        .enumerate()
        .map(|(k, &at)| {
            let end = starts.get(k + 1).copied().unwrap_or(block.len());
            let text = &block[at..end];
            let name: String = text[3..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            (name, text.to_string())
        })
        .collect()
}
