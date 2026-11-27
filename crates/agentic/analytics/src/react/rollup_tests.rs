//! `query`'s rollup tier, with no Parquet and no warehouse: a manifest and
//! stand-in files in a scratch cache directory (as the FSM's
//! `preagg_liveness_tests` do), a fake connector, and a fake rollup reader.
//! Which tier answers is decided before a byte of Parquet is read, and these
//! check that decision and what the `query_executed` row says about it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use agentic_connector::{DatabaseConnector, ExecutionResult, ResultSummary};
use agentic_core::react::{Tool, ToolReply};
use agentic_core::result::{CellValue, QueryResult, QueryRow};
use agentic_semantic::compile::PreaggContext;
use agentic_semantic::refresh_key_cache::RefreshKeyCache;
use async_trait::async_trait;
use serde_json::{Value, json};

use super::files::{FileEntry, FileKind, WorkspaceFiles};
use super::query_tests::{Fake, cx};
use super::tools::{QueryDeps, QueryTool, RollupPlan, RollupReader, Rollups};
use super::verified::VerifiedQueries;
use crate::SemanticCatalog;

/// The shipped definition, with one rollup: `total_amount` by `status` per
/// month. `region` is not in it.
const ORDERS_V1: &str = r#"
name: orders
datasource: local
table: orders
dimensions:
  - name: status
    type: string
    expr: status
  - name: region
    type: string
    expr: region
  - name: order_date
    type: date
    expr: order_date
measures:
  - name: total_amount
    type: sum
    expr: amount
pre_aggregations:
  - name: by_month
    dimensions: [status]
    measures: [total_amount]
    time_dimension: order_date
    granularity: month
"#;

/// The same rollup after an edit that renames nothing: its hash moves, so a
/// V1 manifest row no longer describes a live rollup.
fn orders_v2() -> String {
    ORDERS_V1.replace("expr: amount", "expr: amount - refunds")
}

/// A month-grain `total_amount` by `status`: what `by_month` covers.
fn covered() -> Value {
    json!({"semantic": {
        "measures": ["orders.total_amount"],
        "dimensions": ["orders.status"],
        "time_dimensions": [{"dimension": "orders.order_date", "granularity": "month"}]
    }})
}

/// The rollup's answer: one row.
struct Reader {
    fail: bool,
    plans: Mutex<Vec<RollupPlan>>,
}

impl Reader {
    fn new(fail: bool) -> Arc<Self> {
        Arc::new(Self {
            fail,
            plans: Mutex::new(Vec::new()),
        })
    }

    fn reads(&self) -> usize {
        self.plans.lock().unwrap().len()
    }
}

#[async_trait]
impl RollupReader for Reader {
    async fn read(&self, plan: RollupPlan, _limit: u64) -> Result<ExecutionResult, String> {
        self.plans.lock().unwrap().push(plan);
        if self.fail {
            return Err("not a parquet file".into());
        }
        Ok(ExecutionResult {
            result: QueryResult {
                columns: vec!["status".into(), "total_amount".into()],
                rows: vec![QueryRow(vec![
                    CellValue::Text("done".into()),
                    CellValue::Number(42.0),
                ])],
                total_row_count: 1,
                truncated: false,
            },
            summary: ResultSummary {
                row_count: 1,
                columns: Vec::new(),
            },
        })
    }
}

/// One throwaway workspace's cache directory, removed on drop. It is where
/// `try_resolve_preagg` reads, so the shipped lookup runs.
struct Scratch {
    id: uuid::Uuid,
    dir: std::path::PathBuf,
}

impl Scratch {
    /// A cache holding the manifest a build of `yaml`'s rollups leaves, with
    /// a stand-in (unreadable) Parquet beside each row.
    fn built_from(yaml: &str) -> Self {
        let id = uuid::Uuid::new_v4();
        let dir = oxy_shared::state_dir::get_airlayer_cache_dir(id);
        std::fs::create_dir_all(&dir).expect("cache dir");
        let scratch = Self { id, dir };
        scratch.write_manifest(&view(yaml));
        scratch
    }

    fn write_manifest(&self, view: &oxy_airlayer_compat::View) {
        let rollups: Vec<_> = oxy_airlayer_compat::preagg::resolve_rollups(view)
            .expect("fixture rollups resolve")
            .into_iter()
            .map(|spec| self.entry(view, spec))
            .collect();
        let manifest = oxy_airlayer_compat::preagg::LocalManifest {
            pulled_at: "2026-03-01T00:00:00Z".to_string(),
            source_database: "local".to_string(),
            rollups,
        };
        let json = serde_json::to_string(&manifest).expect("manifest serializes");
        std::fs::write(self.dir.join("manifest.json"), json).expect("manifest writes");
    }

    fn entry(
        &self,
        view: &oxy_airlayer_compat::View,
        spec: oxy_airlayer_compat::preagg::RollupSpec,
    ) -> oxy_airlayer_compat::preagg::LocalRollupEntry {
        let file = format!("{}__{}.parquet", view.name, spec.hash);
        std::fs::write(self.dir.join(&file), b"stand-in").expect("parquet writes");
        let measures = spec
            .measures
            .iter()
            .map(|m| {
                let kind = m.measure_type.to_string();
                json!({"name": m.name, "type": kind, "columns": m.columns})
            })
            .collect();
        oxy_airlayer_compat::preagg::LocalRollupEntry {
            view_name: view.name.clone(),
            rollup_name: spec.name.clone(),
            rollup_hash: spec.hash.clone(),
            file,
            dimensions: spec.dimensions.clone(),
            measures,
            time_dimension: spec.time_dimension.clone(),
            granularity: spec.granularity.clone(),
            build_date: "2026-03-01 00:00:00".to_string(),
            refresh_key_value: None,
            refresh_key_checked_at: None,
        }
    }

    fn context(&self) -> PreaggContext {
        PreaggContext {
            workspace_id: self.id,
            cache: Arc::new(RwLock::new(RefreshKeyCache::new())),
            renewal_threshold_secs: 0,
            blob: None,
            // The read-surface posture, as a served chat has it.
            require_fresh: false,
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn view(yaml: &str) -> oxy_airlayer_compat::View {
    serde_yaml::from_str(yaml).expect("fixture view parses")
}

fn catalog(yaml: &str) -> SemanticCatalog {
    let layer = oxy_airlayer_compat::SemanticLayer::new(vec![view(yaml)], None);
    let dialects = oxy_airlayer_compat::DatasourceDialectMap::with_default(
        oxy_airlayer_compat::Dialect::DuckDB,
    );
    SemanticCatalog::from_engine(
        oxy_airlayer_compat::SemanticEngine::from_semantic_layer(layer, dialects)
            .expect("engine builds"),
    )
}

fn tool(db: &Arc<Fake>, schema: &str, rollups: Option<Rollups>) -> QueryTool {
    tool_with(db, schema, rollups, VerifiedQueries::default())
}

fn tool_with(
    db: &Arc<Fake>,
    schema: &str,
    rollups: Option<Rollups>,
    verified: VerifiedQueries,
) -> QueryTool {
    let connectors: HashMap<String, Arc<dyn DatabaseConnector>> = HashMap::from([(
        "local".to_string(),
        db.clone() as Arc<dyn DatabaseConnector>,
    )]);
    QueryTool::new(QueryDeps {
        connectors,
        catalog: Some(Arc::new(catalog(schema))),
        default_connector: None,
        semantic_only: false,
        verified: Arc::new(verified),
        rollups,
    })
}

fn rollups(scratch: &Scratch, reader: &Arc<Reader>) -> Option<Rollups> {
    Some(Rollups::with_reader(scratch.context(), reader.clone()))
}

async fn call(t: &QueryTool, args: Value) -> ToolReply {
    t.call(args, &cx()).await.expect("query runs")
}

fn is_preagg(reply: &ToolReply) -> bool {
    reply.artifacts[0].payload["is_preagg"].as_bool().unwrap()
}

/// A covered semantic query is answered by the rollup, the warehouse is not
/// asked, and the row badges it. The stored result and the reply carry the
/// warehouse SQL, as the FSM's `query_executed` row does.
#[tokio::test]
async fn a_covered_semantic_query_is_served_from_the_rollup() {
    let scratch = Scratch::built_from(ORDERS_V1);
    let (db, reader) = (Fake::new(3), Reader::new(false));
    let t = tool(&db, ORDERS_V1, rollups(&scratch, &reader));
    let cx = cx();
    let reply = t.call(covered(), &cx).await.expect("query runs");
    assert!(is_preagg(&reply), "{}", reply.text);
    assert_eq!(reader.reads(), 1);
    assert!(
        db.seen().is_empty(),
        "the warehouse was asked: {:?}",
        db.seen()
    );
    let read = reader.plans.lock().unwrap()[0].sql.clone();
    assert!(read.contains("read_parquet"), "{read}");
    let sql = reply.value["sql"].as_str().unwrap();
    assert!(!sql.contains("read_parquet"), "{sql}");
    assert_eq!(reply.artifacts[0].payload["query"], sql);
    assert_eq!(cx.results.sql_for("r1").as_deref(), Some(sql));
    assert!(reply.text.contains("42"), "{}", reply.text);
}

/// The parity cases' shape: a year as an `in_date_range` filter on the
/// rollup's time dimension, with the month grain and without it.
#[tokio::test]
async fn a_year_filter_on_the_time_dimension_is_still_covered() {
    let scratch = Scratch::built_from(ORDERS_V1);
    let year = json!({"field": "orders.order_date", "op": "in_date_range",
        "from": "2025-01-01", "to": "2025-12-31"});
    let mut by_month = covered();
    by_month["semantic"]["filters"] = json!([year.clone()]);
    let by_status = json!({"semantic": {
        "measures": ["orders.total_amount"],
        "dimensions": ["orders.status"],
        "filters": [year]
    }});
    for args in [by_month, by_status] {
        let (db, reader) = (Fake::new(3), Reader::new(false));
        let t = tool(&db, ORDERS_V1, rollups(&scratch, &reader));
        let reply = call(&t, args.clone()).await;
        assert!(is_preagg(&reply), "{args}: {}", reply.text);
        assert!(db.seen().is_empty(), "{args}");
    }
}

#[tokio::test]
async fn an_uncovered_semantic_query_goes_to_the_warehouse() {
    let scratch = Scratch::built_from(ORDERS_V1);
    let (db, reader) = (Fake::new(3), Reader::new(false));
    let t = tool(&db, ORDERS_V1, rollups(&scratch, &reader));
    let args = json!({"semantic": {
        "measures": ["orders.total_amount"],
        "dimensions": ["orders.region"]
    }});
    let reply = call(&t, args).await;
    assert!(!is_preagg(&reply));
    assert_eq!(reader.reads(), 0);
    assert_eq!(db.seen().len(), 1);
}

/// A rollup built from a since-edited definition still matches by name; the
/// live set declines it.
#[tokio::test]
async fn a_rollup_the_schema_no_longer_declares_is_declined() {
    let scratch = Scratch::built_from(ORDERS_V1);
    let (db, reader) = (Fake::new(3), Reader::new(false));
    let t = tool(&db, &orders_v2(), rollups(&scratch, &reader));
    let reply = call(&t, covered()).await;
    assert!(!is_preagg(&reply));
    assert_eq!(reader.reads(), 0);
    assert_eq!(db.seen().len(), 1);
}

/// "The badge has to report what actually answered."
#[tokio::test]
async fn a_rollup_that_will_not_read_falls_back_to_the_warehouse() {
    let scratch = Scratch::built_from(ORDERS_V1);
    let (db, reader) = (Fake::new(3), Reader::new(true));
    let t = tool(&db, ORDERS_V1, rollups(&scratch, &reader));
    let reply = call(&t, covered()).await;
    assert!(!is_preagg(&reply));
    assert_eq!(reader.reads(), 1);
    assert_eq!(db.seen(), vec![reply.value["sql"].as_str().unwrap()]);
    assert!(!reply.text.contains("rollup"), "{}", reply.text);
}

/// The shipped reader, over the stand-in bytes: DuckDB refuses the file and
/// the warehouse answers.
#[tokio::test]
async fn the_duckdb_reader_falls_back_on_an_unreadable_parquet() {
    let scratch = Scratch::built_from(ORDERS_V1);
    let db = Fake::new(3);
    let t = tool(&db, ORDERS_V1, Some(Rollups::new(scratch.context())));
    let reply = call(&t, covered()).await;
    assert!(!is_preagg(&reply));
    assert_eq!(db.seen().len(), 1);
}

/// A verified query on `local`.
fn by_status() -> VerifiedQueries {
    let path = "example_sql/by_status.sql";
    let entry = FileEntry {
        path: path.into(),
        kind: FileKind::from_name(path),
        content: "/* oxy:\n  database: local\n*/\nSELECT status FROM orders".into(),
    };
    VerifiedQueries::from_files(&WorkspaceFiles::from_entries(vec![entry]))
}

/// Raw `sql` and a `verified` query run as written, on the warehouse, with a
/// covering rollup built.
#[tokio::test]
async fn raw_sql_and_verified_queries_never_read_a_rollup() {
    let scratch = Scratch::built_from(ORDERS_V1);
    let (db, reader) = (Fake::new(3), Reader::new(false));
    let t = tool_with(&db, ORDERS_V1, rollups(&scratch, &reader), by_status());
    let sql = call(&t, json!({"sql": "select status from orders"})).await;
    let ver = call(&t, json!({"verified": "example_sql/by_status.sql"})).await;
    assert!(!is_preagg(&sql) && !is_preagg(&ver));
    assert_eq!(reader.reads(), 0);
    assert_eq!(db.seen().len(), 2);
}

/// No preagg context: the covered query runs on the warehouse, and the
/// schema is the one a run without rollups always had.
#[tokio::test]
async fn without_rollups_nothing_changes() {
    let scratch = Scratch::built_from(ORDERS_V1);
    let (db, reader) = (Fake::new(3), Reader::new(false));
    let without = tool(&db, ORDERS_V1, None);
    let with = tool(&db, ORDERS_V1, rollups(&scratch, &reader));
    assert_eq!(without.def().parameters, with.def().parameters);
    assert_eq!(without.def().description, with.def().description);
    let reply = call(&without, covered()).await;
    assert!(!is_preagg(&reply));
    assert!(!reply.text.contains("rollup"), "{}", reply.text);
    assert_eq!(db.seen().len(), 1);
}
