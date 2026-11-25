//! `query` against an in-memory connector that records every SQL string.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use agentic_connector::{
    ConnectorError, DatabaseConnector, ExecutionResult, ResultSummary, SqlDialect,
};
use agentic_core::react::{Tool, ToolCx, ToolFailure, ToolReply};
use agentic_core::result::{CellValue, QueryResult, QueryRow};
use agentic_runtime::react::ResultStore;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::tools::{EVENT_PREVIEW_ROWS, QueryDeps, QueryTool};
use crate::SemanticCatalog;

/// Table `t(id, name)` of `rows` rows; refuses nothing, records everything.
pub(super) struct Fake {
    rows: usize,
    /// Report the shown row count as the total, as a connector that cannot
    /// count the full result does.
    unknown_total: bool,
    fail: Option<String>,
    seen: Mutex<Vec<String>>,
    /// Which connector method ran, in order.
    methods: Mutex<Vec<&'static str>>,
}

impl Fake {
    pub(super) fn new(rows: usize) -> Arc<Self> {
        Arc::new(Self {
            rows,
            unknown_total: false,
            fail: None,
            seen: Mutex::new(Vec::new()),
            methods: Mutex::new(Vec::new()),
        })
    }
    pub(super) fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
    fn methods(&self) -> Vec<&'static str> {
        self.methods.lock().unwrap().clone()
    }

    fn answer(&self, sql: &str, limit: u64) -> Result<ExecutionResult, ConnectorError> {
        self.seen.lock().unwrap().push(sql.to_owned());
        if let Some(m) = &self.fail {
            return Err(ConnectorError::query_failed(sql, m.clone()));
        }
        let rows: Vec<QueryRow> = (0..self.rows)
            .take(limit as usize)
            .map(|i| {
                QueryRow(vec![
                    CellValue::Number(i as f64),
                    CellValue::Text(format!("n{i}")),
                ])
            })
            .collect();
        let total = self.rows as u64;
        let truncated = (rows.len() as u64) < total;
        let total = if self.unknown_total {
            rows.len() as u64
        } else {
            total
        };
        Ok(ExecutionResult {
            result: QueryResult {
                columns: vec!["id".into(), "name".into()],
                truncated,
                rows,
                total_row_count: total,
            },
            summary: ResultSummary {
                row_count: total,
                columns: Vec::new(),
            },
        })
    }
}

#[async_trait::async_trait]
impl DatabaseConnector for Fake {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::Postgres
    }

    async fn execute_query(
        &self,
        sql: &str,
        limit: u64,
    ) -> Result<ExecutionResult, ConnectorError> {
        self.methods.lock().unwrap().push("execute_query");
        self.answer(sql, limit)
    }

    fn supports_read_only(&self) -> bool {
        true
    }

    async fn execute_query_read_only(
        &self,
        sql: &str,
        limit: u64,
    ) -> Result<ExecutionResult, ConnectorError> {
        self.methods.lock().unwrap().push("execute_query_read_only");
        self.answer(sql, limit)
    }
}

pub(super) fn cx() -> ToolCx {
    ToolCx {
        run_id: "r".into(),
        call_id: "c".into(),
        cancel: CancellationToken::new(),
        results: Arc::new(ResultStore::new()),
    }
}

pub(super) fn tool(conns: &[(&str, Arc<Fake>)], catalog: Option<SemanticCatalog>) -> QueryTool {
    let connectors: HashMap<String, Arc<dyn DatabaseConnector>> = conns
        .iter()
        .map(|(n, c)| (n.to_string(), c.clone() as Arc<dyn DatabaseConnector>))
        .collect();
    QueryTool::new(QueryDeps {
        connectors,
        catalog: catalog.map(Arc::new),
        default_connector: None,
        semantic_only: false,
    })
}

pub(super) async fn run(t: &QueryTool, args: Value) -> Result<ToolReply, ToolFailure> {
    t.call(args, &cx()).await
}

pub(super) fn account_catalog() -> SemanticCatalog {
    let view = crate::airlayer_compat::parse_view_yaml(
        r#"
name: account
table: account
entities:
  - name: account_id
    type: primary
    key: account_id
dimensions:
  - name: account_id
    type: number
    expr: account_id
  - name: district_id
    type: number
    expr: district_id
measures:
  - name: total_amount
    type: sum
    expr: amount
"#,
    )
    .unwrap();
    let layer = oxy_airlayer_compat::SemanticLayer::new(vec![view], None);
    let dialects = oxy_airlayer_compat::DatasourceDialectMap::with_default(
        oxy_airlayer_compat::Dialect::DuckDB,
    );
    let engine = oxy_airlayer_compat::SemanticEngine::from_semantic_layer(layer, dialects).unwrap();
    SemanticCatalog::from_engine(engine)
}

#[tokio::test]
async fn query_runs_sql_and_stores_the_result() {
    let db = Fake::new(30);
    let t = tool(&[("warehouse", db.clone())], None);
    let reply = run(&t, json!({"sql": "select * from t"})).await.unwrap();
    assert!(
        reply.text.starts_with("result r1: 30 rows\n"),
        "{}",
        reply.text
    );
    assert!(
        reply.text.contains("19 | n19\n… 10 more rows"),
        "{}",
        reply.text
    );
    assert!(!reply.text.contains("| n20"), "{}", reply.text);
    assert_eq!(
        reply.value,
        json!({"result_id": "r1", "sql": "select * from t"})
    );
    assert_eq!(db.seen(), vec!["select * from t"]);

    let [a] = reply.artifacts.as_slice() else {
        panic!("one artifact: {:?}", reply.artifacts)
    };
    assert_eq!(a.event_type, "query_executed");
    let p = &a.payload;
    assert_eq!(p["query"], "select * from t");
    assert_eq!(p["row_count"], 30);
    assert_eq!(
        p["rows"].as_array().unwrap().len(),
        EVENT_PREVIEW_ROWS,
        "the row carries a preview; row_count is the full count"
    );
    assert_eq!(p["rows"][3], json!([3.0, "n3"]));
    assert_eq!(p["columns"], json!(["id", "name"]));
    assert_eq!(p["success"], true);
    assert_eq!(p["error"], Value::Null);
    assert_eq!(p["source"], "llm");
    assert_eq!(p["is_preagg"], false);
    assert!(
        p.get("sub_spec_index").is_none() && p.get("semantic_query").is_none(),
        "{p}"
    );
}

/// The tool's only path to the warehouse is the connector's read-only one,
/// where the database itself refuses a write; never `execute_query`.
#[tokio::test]
async fn query_runs_on_the_read_only_path() {
    let db = Fake::new(3);
    let t = tool(&[("warehouse", db.clone())], None);
    run(&t, json!({"sql": "select * from t"})).await.unwrap();
    assert_eq!(db.methods(), vec!["execute_query_read_only"]);
}

#[tokio::test]
async fn query_preview_artifact_is_capped_at_the_event_preview() {
    let t = tool(&[("warehouse", Fake::new(150))], None);
    let reply = run(&t, json!({"sql": "select * from t", "limit": 500}))
        .await
        .unwrap();
    let p = &reply.artifacts[0].payload;
    assert_eq!(p["row_count"], 150);
    assert_eq!(p["rows"].as_array().unwrap().len(), EVENT_PREVIEW_ROWS);
    // `limit` is clamped to 100 rows shown.
    assert!(reply.text.ends_with("… 50 more rows"), "{}", reply.text);
}

#[tokio::test]
async fn query_honours_a_smaller_limit() {
    let t = tool(&[("warehouse", Fake::new(30))], None);
    let reply = run(&t, json!({"sql": "select * from t", "limit": 5}))
        .await
        .unwrap();
    assert!(
        reply.text.ends_with("4 | n4\n… 25 more rows"),
        "{}",
        reply.text
    );
}

#[tokio::test]
async fn query_refuses_a_write_before_executing() {
    let db = Fake::new(30);
    let t = tool(&[("warehouse", db.clone())], None);
    let err = run(&t, json!({"sql": "delete from t"})).await.unwrap_err();
    assert!(err.error.contains("read-only"), "{}", err.error);
    assert!(db.seen().is_empty(), "the connector saw {:?}", db.seen());
}

#[tokio::test]
async fn query_needs_exactly_one_of_sql_and_semantic() {
    let t = tool(&[("warehouse", Fake::new(1))], Some(account_catalog()));
    for args in [
        json!({}),
        json!({"sql": "select 1", "semantic": {"measures": ["account.total_amount"]}}),
    ] {
        let err = run(&t, args).await.unwrap_err();
        assert!(err.error.contains("exactly one"), "{}", err.error);
    }
}

#[tokio::test]
async fn query_failure_is_an_ok_reply_with_a_failed_artifact() {
    let db = Arc::new(Fake {
        rows: 0,
        unknown_total: false,
        fail: Some("column \"x\" does not exist".into()),
        seen: Mutex::new(Vec::new()),
        methods: Mutex::new(Vec::new()),
    });
    let t = tool(&[("warehouse", db)], None);
    let reply = run(&t, json!({"sql": "select x from t"})).await.unwrap();
    assert!(reply.text.starts_with("query failed:"), "{}", reply.text);
    assert!(reply.text.contains("does not exist"), "{}", reply.text);
    assert_eq!(reply.value, Value::Null);
    let p = &reply.artifacts[0].payload;
    assert_eq!(p["success"], false);
    assert_eq!(p["row_count"], 0);
    assert_eq!(p["rows"], json!([]));
    assert!(
        p["error"].as_str().unwrap().contains("does not exist"),
        "{p}"
    );
}

#[tokio::test]
async fn query_validates_arguments_before_resolving_the_database() {
    // Two connectors and no `database`: the argument error still wins.
    let t = tool(
        &[("warehouse", Fake::new(1)), ("sales", Fake::new(1))],
        None,
    );
    let err = run(&t, json!({})).await.unwrap_err();
    assert!(err.error.contains("exactly one"), "{}", err.error);
    for limit in [json!(0), json!("ten"), json!(-3)] {
        let err = run(&t, json!({"sql": "select 1", "limit": limit}))
            .await
            .unwrap_err();
        assert!(err.error.contains("limit"), "{}", err.error);
    }
}

#[tokio::test]
async fn query_refuses_gate_bypass_payloads_before_executing() {
    let db = Fake::new(30);
    let t = tool(&[("warehouse", db.clone())], None);
    for (sql, hint) in [
        (
            "select 1 # ; delete from t",
            "only a single SELECT or WITH query runs",
        ),
        (
            "select 1 /*! ; delete from t */",
            "only a single SELECT or WITH query runs",
        ),
        (
            r"select E'\'' ); delete from t; select 1 -- '",
            super::read_only::ESCAPE_STRING_HINT,
        ),
    ] {
        let err = run(&t, json!({"sql": sql})).await.unwrap_err();
        assert_eq!(err.hint.as_deref(), Some(hint), "{sql}: {err:?}");
    }
    assert!(db.seen().is_empty(), "the connector saw {:?}", db.seen());
}

#[tokio::test]
async fn query_fails_closed_on_sql_the_parser_cannot_read() {
    let db = Fake::new(30);
    let t = tool(&[("warehouse", db.clone())], None);
    for sql in [
        "select 1 from t where x = )",
        "select 1 from t where x = ) delete from t",
    ] {
        let err = run(&t, json!({"sql": sql})).await.unwrap_err();
        assert_eq!(
            err.hint.as_deref(),
            Some("rewrite as a single standard SELECT"),
            "{sql}: {err:?}"
        );
    }
    assert!(db.seen().is_empty(), "the connector saw {:?}", db.seen());
}

#[tokio::test]
async fn query_truncated_result_names_the_total_when_the_connector_knows_it() {
    let db = Fake::new(15_000);
    let t = tool(&[("warehouse", db)], None);
    let reply = run(&t, json!({"sql": "select * from t"})).await.unwrap();
    assert!(
        reply
            .text
            .starts_with("result r1: 10000 of 15000 rows (truncated)\n"),
        "{}",
        reply.text
    );
    let payload = &reply.artifacts[0].payload;
    assert_eq!(
        payload["row_count"],
        json!(10_000),
        "artifact row_count stays the rows held"
    );

    let db = Arc::new(Fake {
        rows: 15_000,
        unknown_total: true,
        fail: None,
        seen: Mutex::new(Vec::new()),
        methods: Mutex::new(Vec::new()),
    });
    let t = tool(&[("warehouse", db)], None);
    let reply = run(&t, json!({"sql": "select * from t"})).await.unwrap();
    assert!(
        reply
            .text
            .starts_with("result r1: 10000 rows (truncated)\n"),
        "{}",
        reply.text
    );
}
