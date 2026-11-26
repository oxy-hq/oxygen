//! The freshness probes read the relation a view's `table:` names, quoted one
//! identifier per part, and never query a `sql:` view by its name.
//!
//! Before this, `amazon_sp.fba_inventory` was quoted whole as
//! `"amazon_sp.fba_inventory"`, a table whose name contains a dot. DuckDB
//! answered "does not exist", or, under a read-only Airhouse credential, tried
//! the name as a file and was refused. A `sql:` view was probed by its view
//! name, which names no relation at all.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use agentic_connector::{ConnectorError, DatabaseConnector, ExecutionResult, SqlDialect};
use serde_json::json;

use super::execute_specifying_tool;

/// Records every statement it is sent and answers each with no rows.
#[derive(Default)]
struct Recording(Mutex<Vec<String>>);

#[async_trait::async_trait]
impl DatabaseConnector for Recording {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::DuckDb
    }

    async fn execute_query(
        &self,
        sql: &str,
        _limit: u64,
    ) -> Result<ExecutionResult, ConnectorError> {
        self.0.lock().unwrap().push(sql.to_string());
        Ok(ExecutionResult::empty())
    }
}

const TABLE_VIEW: &str = r#"
name: fba_inventory
description: FBA inventory
table: amazon_sp.fba_inventory
datasource: bmg
meta:
  freshness_watermark_column: ["_aw_pulled_at"]
dimensions:
  - name: sku
    type: string
    expr: sku
"#;

const SQL_VIEW: &str = r#"
name: reorder
description: Reorder sheet
sql: SELECT 'a' AS sku, DATE '2026-11-20' AS as_of_date
datasource: bmg
meta:
  freshness_watermark_column: ["as_of_date"]
dimensions:
  - name: sku
    type: string
    expr: sku
  - name: as_of_date
    type: date
    expr: as_of_date
"#;

fn catalog(yamls: &[&str]) -> crate::SemanticCatalog {
    let views = yamls
        .iter()
        .map(|y| crate::airlayer_compat::parse_view_yaml(y).unwrap())
        .collect();
    let layer = oxy_airlayer_compat::SemanticLayer::new(views, None);
    let dialects = oxy_airlayer_compat::DatasourceDialectMap::with_default(
        oxy_airlayer_compat::Dialect::DuckDB,
    );
    let engine = oxy_airlayer_compat::SemanticEngine::from_semantic_layer(layer, dialects).unwrap();
    crate::SemanticCatalog::from_engine(engine)
}

fn bmg(rec: &Arc<Recording>) -> HashMap<String, Arc<dyn DatabaseConnector>> {
    HashMap::from([("bmg".to_string(), rec.clone() as Arc<dyn DatabaseConnector>)])
}

#[tokio::test]
async fn check_data_freshness_quotes_a_schema_qualified_table_per_part() {
    let rec = Arc::new(Recording::default());
    let out = execute_specifying_tool(
        "check_data_freshness",
        json!({"views": ["fba_inventory"]}),
        &catalog(&[TABLE_VIEW]),
        &bmg(&rec),
        "bmg",
    )
    .await
    .unwrap();
    let sent = rec.0.lock().unwrap().clone();
    assert_eq!(
        sent,
        vec![r#"SELECT MAX(_aw_pulled_at) FROM "amazon_sp"."fba_inventory""#.to_string()],
        "{out}"
    );
}

#[tokio::test]
async fn check_data_freshness_does_not_query_a_sql_view_by_its_name() {
    let rec = Arc::new(Recording::default());
    let out = execute_specifying_tool(
        "check_data_freshness",
        json!({"views": ["reorder"]}),
        &catalog(&[SQL_VIEW]),
        &bmg(&rec),
        "bmg",
    )
    .await
    .unwrap();
    assert!(
        rec.0.lock().unwrap().is_empty(),
        "nothing is queried: {out}"
    );
    let text = out.to_string();
    assert!(
        text.contains("defined by SQL"),
        "the answer says why: {text}"
    );
    assert!(
        !text.contains("\"error\""),
        "a skipped probe is not an error: {text}"
    );
}

#[tokio::test]
async fn the_ambient_hint_probes_table_views_and_skips_sql_views() {
    let rec = Arc::new(Recording::default());
    let hint = crate::compute_freshness_hint(
        "freshness-probe-tests-ambient",
        &catalog(&[TABLE_VIEW, SQL_VIEW]),
        &bmg(&rec),
        "bmg",
    )
    .await;
    // The recording connector returns no rows, so no line is produced; what
    // is pinned is what was sent.
    assert!(hint.is_none());
    let sent = rec.0.lock().unwrap().clone();
    assert_eq!(
        sent,
        vec![r#"SELECT MAX(_aw_pulled_at) FROM "amazon_sp"."fba_inventory""#.to_string()]
    );
}
