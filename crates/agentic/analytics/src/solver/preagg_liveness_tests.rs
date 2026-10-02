//! A rollup whose definition has moved must not answer the analytics agent.
//!
//! `covers()` matches a manifest row by member *names* — `entry.dimensions`,
//! `entry.measures`, the time dimension and its grain. Nothing on that path
//! reads the stored Parquet's schema, so an edit that renames nothing
//! (`expr: amount` → `expr: amount - refunds`, `type: sum` → `type: avg`,
//! a new `filters:` entry, a repointed `table:`) leaves every name `covers()`
//! inspects identical while moving the rollup's hash. The guard that catches
//! that is the `live` set — the `(view, hash)` pairs the schema declares right
//! now — and passing `None` for it is what restores name-only matching.
//!
//! So these two tests are a pair and neither is meaningful alone. One pins the
//! wrong answer: an edited definition must send the query to the warehouse.
//! The other pins the cost of over-correcting: an UNedited definition must
//! still be served from its rollup, because the cheapest way to close this
//! hole is to decline everything, and that silently turns pre-aggregation off
//! for the analytics agent.

#![cfg(test)]

use std::sync::{Arc, RwLock};

use agentic_semantic::compile::PreaggContext;
use agentic_semantic::refresh_key_cache::RefreshKeyCache;
use async_trait::async_trait;

use super::AnalyticsSolver;
use crate::{LlmClient, SemanticCatalog, SolutionPayload};

/// The shipped definition: `total_amount` sums `amount`.
const ORDERS_V1: &str = r#"
name: orders
datasource: local
table: orders
dimensions:
  - name: status
    type: string
    expr: status
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

/// The same rollup after an edit that renames nothing: `total_amount` now nets
/// refunds out. Every name `covers()` looks at is unchanged — view, rollup,
/// dimension, measure, time dimension, granularity — so the V1 row matches
/// this schema's queries exactly as before, while holding the pre-refund
/// numbers.
const ORDERS_V2: &str = r#"
name: orders
datasource: local
table: orders
dimensions:
  - name: status
    type: string
    expr: status
  - name: order_date
    type: date
    expr: order_date
measures:
  - name: total_amount
    type: sum
    expr: amount - refunds
pre_aggregations:
  - name: by_month
    dimensions: [status]
    measures: [total_amount]
    time_dimension: order_date
    granularity: month
"#;

struct StubConnector;

#[async_trait]
impl agentic_connector::DatabaseConnector for StubConnector {
    fn dialect(&self) -> agentic_connector::SqlDialect {
        agentic_connector::SqlDialect::DuckDb
    }

    async fn execute_query(
        &self,
        _sql: &str,
        _limit: u64,
    ) -> Result<agentic_connector::ExecutionResult, agentic_connector::ConnectorError> {
        Err(agentic_connector::ConnectorError::Other("stub".into()))
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

/// One throwaway workspace's cache directory, removed on drop.
///
/// Writes where `try_resolve_preagg` reads — the process-wide state dir — so
/// these exercise the shipped lookup rather than a parallel one. `Drop` rather
/// than a tail cleanup so a failing assertion leaves no debris in a
/// developer's real state dir.
struct ScratchWorkspace {
    id: uuid::Uuid,
    dir: std::path::PathBuf,
}

impl ScratchWorkspace {
    fn new() -> Self {
        let id = uuid::Uuid::new_v4();
        let dir = oxy_shared::state_dir::get_airlayer_cache_dir(id);
        std::fs::create_dir_all(&dir).expect("cache dir");
        Self { id, dir }
    }
}

impl Drop for ScratchWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Write the manifest a build of `yaml`'s rollups would have left behind,
/// with a stand-in Parquet beside each row so `resolve_source` finds the local
/// tier. The files are never read — every assertion here is about which tier
/// was *chosen*, which is decided before a byte of Parquet is opened.
fn build_manifest_from(workspace: &ScratchWorkspace, yaml: &str) {
    let view = view(yaml);
    let rollups: Vec<oxy_airlayer_compat::preagg::LocalRollupEntry> =
        oxy_airlayer_compat::preagg::resolve_rollups(&view)
            .into_iter()
            .map(|spec| {
                let file = format!("{}__{}.parquet", view.name, spec.hash);
                std::fs::write(workspace.dir.join(&file), b"stand-in").expect("parquet writes");
                oxy_airlayer_compat::preagg::LocalRollupEntry {
                    view_name: view.name.clone(),
                    rollup_name: spec.name.clone(),
                    rollup_hash: spec.hash.clone(),
                    file,
                    dimensions: spec.dimensions.clone(),
                    measures: spec
                        .measures
                        .iter()
                        .map(|m| {
                            serde_json::json!({
                                "name": m.name,
                                "type": m.measure_type.to_string(),
                                "columns": m.columns,
                            })
                        })
                        .collect(),
                    time_dimension: spec.time_dimension.clone(),
                    granularity: spec.granularity.clone(),
                    build_date: "2026-03-01 00:00:00".to_string(),
                    refresh_key_value: None,
                    refresh_key_checked_at: None,
                }
            })
            .collect();

    let manifest = oxy_airlayer_compat::preagg::LocalManifest {
        pulled_at: "2026-03-01T00:00:00Z".to_string(),
        source_database: "local".to_string(),
        rollups,
    };
    std::fs::write(
        workspace.dir.join("manifest.json"),
        serde_json::to_string(&manifest).expect("manifest serializes"),
    )
    .expect("manifest writes");
}

fn preagg_ctx(workspace: &ScratchWorkspace) -> PreaggContext {
    PreaggContext {
        workspace_id: workspace.id,
        cache: Arc::new(RwLock::new(RefreshKeyCache::new())),
        // 0 means "trust nothing cached", which keeps freshness out of what
        // these tests measure.
        renewal_threshold_secs: 0,
        blob: None,
        // `true` would decline on the cold cache alone and both tests would
        // pass for the wrong reason.
        require_fresh: false,
    }
}

/// The query the agent asks: a month-grain `total_amount` by `status`. Every
/// member it names exists under both definitions, which is the whole point —
/// name-based coverage cannot tell the two apart.
fn request() -> oxy_airlayer_compat::engine::query::QueryRequest {
    oxy_airlayer_compat::engine::query::QueryRequest {
        measures: vec!["orders.total_amount".to_string()],
        dimensions: vec!["orders.status".to_string()],
        time_dimensions: vec![oxy_airlayer_compat::engine::query::TimeDimensionQuery {
            dimension: "orders.order_date".to_string(),
            granularity: Some("month".to_string()),
            date_range: None,
        }],
        ..Default::default()
    }
}

/// Run the request through the solver's payload builder against `schema_yaml`,
/// with `manifest_yaml`'s rollups already built in the cache.
fn payload_for(manifest_yaml: &str, schema_yaml: &str) -> SolutionPayload {
    let workspace = ScratchWorkspace::new();
    build_manifest_from(&workspace, manifest_yaml);

    let catalog = catalog(schema_yaml);
    let request = request();
    let sql = catalog
        .engine()
        .compile_query(&request)
        .expect("fixture query compiles");
    let sql = crate::airlayer_compat::substitute_params(
        &crate::airlayer_compat::request_dialect(catalog.engine(), &request),
        &sql.sql,
        &sql.params,
    );

    let solver = AnalyticsSolver::new(LlmClient::new("dummy"), catalog, Box::new(StubConnector))
        .with_preagg(Some(preagg_ctx(&workspace)));

    solver.build_semantic_payload(sql, &request)
}

#[test]
fn a_rollup_built_from_an_edited_definition_does_not_answer() {
    let payload = payload_for(ORDERS_V1, ORDERS_V2);
    assert!(
        matches!(payload, SolutionPayload::Sql(_)),
        "a rollup holding the pre-edit numbers answered under the \
         Pre-aggregated badge; the query belongs in the warehouse until the \
         rebuild catches up"
    );
}

#[test]
fn a_rollup_the_schema_still_declares_keeps_answering() {
    let payload = payload_for(ORDERS_V1, ORDERS_V1);
    assert!(
        matches!(payload, SolutionPayload::Preaggregation { .. }),
        "pre-aggregation is off for the analytics agent — declining a rollup \
         the schema still declares is the over-correction, not the fix"
    );
}
