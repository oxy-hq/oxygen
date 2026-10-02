//! The one assertion the "Pre-aggregated" badge is a promise about: a query
//! answered from a local rollup returns the SAME rows as the same query
//! answered from the warehouse.
//!
//! Everything else about pre-aggregation is a performance story. This is the
//! correctness story, and it had no test in this repo — the re-aggregation SQL
//! is generated upstream in airlayer, but the badge that tells a user "same
//! answer, cheaper" is shipped from here, so the equivalence belongs here too.
//!
//! DuckDB stands in for both sides: it is the engine the rollup path uses
//! anyway (`read_parquet`), and it is a supported warehouse dialect, so one
//! connection can execute the warehouse SQL and the re-agg SQL and the
//! comparison is between two real result sets rather than two SQL strings.
//!
//! The non-additive measures are the point of the fixture. `sum`/`count`
//! re-aggregate trivially; `avg` and `count_distinct` do not — averaging an
//! average is wrong, and counting distinct values of an already-grouped column
//! is wrong — so a rollup that stored them naively would disagree with the
//! warehouse exactly here.

#![cfg(test)]

use std::sync::{Arc, RwLock};

use oxy_airlayer_compat::Dialect;
use oxy_airlayer_compat::engine::query::QueryRequest;

use crate::compile::{BlobConfig, CompiledQuery, PreaggContext, PreaggSource, try_resolve_preagg};
use crate::refresh_key_cache::RefreshKeyCache;

/// The source table, one view over it, and one rollup at month grain.
///
/// `order_date` spans two months and every dimension value repeats, so a
/// month-grain rollup genuinely folds rows — an equivalence that held only
/// because each group had one row would prove nothing.
const ORDERS_VIEW: &str = r#"
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
  - name: total_orders
    type: count
  - name: total_amount
    type: sum
    expr: amount
  - name: avg_amount
    type: average
    expr: amount
  - name: distinct_customers
    type: count_distinct
    expr: customer_id
  - name: paid_amount
    type: sum
    expr: amount
    filters:
      - expr: "{{status}} = 'paid'"
pre_aggregations:
  - name: by_month
    dimensions: [status]
    measures:
      [total_orders, total_amount, avg_amount, distinct_customers, paid_amount]
    time_dimension: order_date
    granularity: month
"#;

const SEED_ROWS: &str = "
INSERT INTO orders VALUES
  ('2026-01-04', 'paid',    10.0, 1),
  ('2026-01-11', 'paid',    20.0, 1),
  ('2026-01-18', 'paid',    30.0, 2),
  ('2026-01-04', 'pending',  5.0, 3),
  ('2026-01-22', 'pending', 15.0, 3),
  ('2026-02-02', 'paid',    40.0, 2),
  ('2026-02-09', 'paid',    60.0, 4),
  ('2026-02-14', 'pending', 25.0, 5),
  ('2026-02-27', 'pending', 35.0, 5);
";

fn view() -> oxy_airlayer_compat::View {
    serde_yaml::from_str(ORDERS_VIEW).expect("fixture view should parse")
}

fn layer() -> oxy_airlayer_compat::SemanticLayer {
    oxy_airlayer_compat::SemanticLayer::new(vec![view()], None)
}

fn engine() -> oxy_airlayer_compat::SemanticEngine {
    let dialects = oxy_airlayer_compat::DatasourceDialectMap::with_default(Dialect::DuckDB);
    oxy_airlayer_compat::SemanticEngine::from_semantic_layer(layer(), dialects)
        .expect("engine builds")
}

/// Run a query and return its rows as `Vec<Vec<String>>`, column order
/// preserved, every value stringified.
///
/// Stringifying is deliberate: the two paths can legitimately land on
/// different numeric widths (a `HUGEINT` count from the warehouse, a `BIGINT`
/// read back out of Parquet) for the same number, and this test is about the
/// VALUES agreeing, not the physical types. Floats are rounded to 6 places for
/// the same reason — re-aggregating a decomposed average is a different
/// sequence of additions than averaging the raw column, and the last bits of
/// an f64 are allowed to differ.
fn rows(conn: &duckdb::Connection, sql: &str) -> Vec<Vec<String>> {
    let mut stmt = conn
        .prepare(sql)
        .unwrap_or_else(|e| panic!("prepare failed for {sql}: {e}"));
    let mut out = Vec::new();
    let mut query = stmt.query([]).expect("query runs");
    while let Some(row) = query.next().expect("row reads") {
        let mut cells = Vec::new();
        let mut i = 0;
        while let Ok(value) = row.get::<usize, duckdb::types::Value>(i) {
            cells.push(render(&value));
            i += 1;
        }
        out.push(cells);
    }
    // No ORDER BY is imposed on either side, so compare as sets. That is
    // itself a finding worth pinning: an unordered query with a LIMIT returns
    // an arbitrary subset, and the two paths need not pick the same one — the
    // explorer sends `limit: 1000` by default, so any test that compared
    // limited, unordered results would be flaky by construction rather than
    // wrong. These queries are unlimited.
    out.sort();
    out
}

fn render(value: &duckdb::types::Value) -> String {
    use duckdb::types::Value;
    match value {
        Value::Null => "NULL".to_string(),
        Value::Double(f) => format!("{f:.6}"),
        Value::Float(f) => format!("{f:.6}"),
        Value::Decimal(d) => format!("{d}"),
        // Integer WIDTH legitimately differs between the two paths — DuckDB
        // widens a COUNT over the base table to HUGEINT and a SUM over the
        // rollup's own BIGINT column to BIGINT — so compare the number, not
        // the variant. A width difference is not a wrong answer; letting it
        // fail the test would be a test about DuckDB's type inference.
        Value::TinyInt(n) => n.to_string(),
        Value::SmallInt(n) => n.to_string(),
        Value::Int(n) => n.to_string(),
        Value::BigInt(n) => n.to_string(),
        Value::HugeInt(n) => n.to_string(),
        Value::UTinyInt(n) => n.to_string(),
        Value::USmallInt(n) => n.to_string(),
        Value::UInt(n) => n.to_string(),
        Value::UBigInt(n) => n.to_string(),
        other => format!("{other:?}"),
    }
}

/// One throwaway workspace's cache directory, removed on drop.
///
/// The fixture writes where `try_resolve_preagg` reads — the process-wide
/// state dir — rather than a tempdir, so these tests exercise the shipped
/// lookup rather than a parallel one. `Drop` rather than a tail cleanup so a
/// failing assertion doesn't leave debris in a developer's real state dir.
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

fn preagg_ctx(workspace: &ScratchWorkspace, blob: Option<BlobConfig>) -> PreaggContext {
    PreaggContext {
        workspace_id: workspace.id,
        cache: Arc::new(RwLock::new(RefreshKeyCache::new())),
        // 0 means "trust nothing cached", which keeps freshness out of what
        // these tests are measuring.
        renewal_threshold_secs: 0,
        blob,
        // These tests compare the two tiers' *answers*; declining on staleness
        // would take the rollup out of the comparison entirely.
        require_fresh: false,
    }
}

/// A DuckDB holding the source table, plus the rollup table built from it by
/// airlayer's own build plan, plus that rollup written into the workspace's
/// real cache directory alongside a manifest — exactly what a rebuild leaves
/// behind.
fn fixture(workspace: &ScratchWorkspace) -> duckdb::Connection {
    let conn = duckdb::Connection::open_in_memory().expect("duckdb opens");
    conn.execute_batch(
        "CREATE TABLE orders (order_date DATE, status VARCHAR, amount DOUBLE, customer_id INTEGER);",
    )
    .expect("source table creates");
    conn.execute_batch(SEED_ROWS).expect("seed rows insert");

    let v = view();
    let plan = oxy_airlayer_compat::preagg::collect_build_sql(
        &[&v],
        "main",
        "20260301",
        &Dialect::DuckDB,
        None,
        None,
    )
    .expect("build plan generates");

    for statement in &plan.statements {
        conn.execute_batch(statement)
            .unwrap_or_else(|e| panic!("build statement failed: {e}\n{statement}"));
    }

    let cache_dir = workspace.dir.clone();
    let rollups = plan
        .manifest_entries
        .iter()
        .map(|entry| {
            let file = format!("{}__{}.parquet", entry.view_name, entry.rollup_hash);
            let parquet_path = cache_dir.join(&file);
            let table = entry
                .table_name
                .rsplit_once('.')
                .map(|(_, t)| t.to_string())
                .unwrap_or_else(|| entry.table_name.clone());
            // Export every temporal column as VARCHAR, because that is what
            // production does and this fixture is only worth anything if it
            // writes what production writes. `write_result_to_parquet` builds
            // its Parquet from `VALUES` rows of `CellValue`, whose only
            // non-numeric variant is `Text` — so a bucket column reaches disk
            // as a string. Copying the CTAS table straight across instead kept
            // the built table's DATE/TIMESTAMP, and the divergence was silent
            // until airlayer `aa4bf15` began wrapping the stored bucket in
            // `NULLIF(col, '')`: DuckDB resolves that to the column's type, so
            // on a temporal column it casts `''` to TIMESTAMP and throws on the
            // first row. That wrapper is only valid over VARCHAR and
            // `LocalRollupEntry` does not record the type — filed upstream; the
            // type-agnostic form is `CAST(NULLIF(CAST(col AS VARCHAR), '') AS
            // TIMESTAMP)`. Nothing here is a workaround for that bug: this
            // makes the fixture match the writer, which is what it always
            // claimed to do.
            let temporal: Vec<String> = {
                let mut stmt = conn
                    .prepare(&format!("PRAGMA table_info(\"{table}\")"))
                    .expect("table_info prepares");
                let mut rows = stmt.query([]).expect("table_info runs");
                let mut cols = Vec::new();
                while let Some(row) = rows.next().expect("table_info row reads") {
                    let name: String = row.get(1).expect("column name");
                    let ty: String = row.get(2).expect("column type");
                    let ty = ty.to_ascii_uppercase();
                    if ty.starts_with("DATE") || ty.starts_with("TIMESTAMP") {
                        cols.push(name);
                    }
                }
                cols
            };
            let projection = if temporal.is_empty() {
                "*".to_string()
            } else {
                let replaces: Vec<String> = temporal
                    .iter()
                    .map(|c| format!("CAST(\"{c}\" AS VARCHAR) AS \"{c}\""))
                    .collect();
                format!("* REPLACE ({})", replaces.join(", "))
            };
            conn.execute_batch(&format!(
                "COPY (SELECT {projection} FROM \"{table}\") TO '{}' (FORMAT PARQUET);",
                parquet_path.display()
            ))
            .unwrap_or_else(|e| panic!("parquet export failed for {table}: {e}"));
            oxy_airlayer_compat::preagg::LocalRollupEntry {
                view_name: entry.view_name.clone(),
                rollup_name: entry.rollup_name.clone(),
                rollup_hash: entry.rollup_hash.clone(),
                file,
                dimensions: entry.dimensions.clone(),
                measures: serde_json::from_str(&entry.measures_json).expect("measures json"),
                time_dimension: entry.time_dimension.clone(),
                granularity: entry.granularity.clone(),
                build_date: entry.build_date.clone(),
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
        cache_dir.join("manifest.json"),
        serde_json::to_string(&manifest).expect("manifest serializes"),
    )
    .expect("manifest writes");
    conn
}

/// Remove the Parquet files, leaving the manifest — the state every node but
/// the one that ran the rebuild is in.
fn drop_local_parquets(workspace: &ScratchWorkspace) {
    for entry in std::fs::read_dir(&workspace.dir).expect("cache dir reads") {
        let path = entry.expect("dir entry").path();
        if path.extension().is_some_and(|e| e == "parquet") {
            std::fs::remove_file(path).expect("parquet removes");
        }
    }
}

fn warehouse_sql(request: &QueryRequest) -> String {
    let eng = engine();
    let compiled = eng.compile_query(request).expect("warehouse compile");
    oxy_airlayer_compat::substitute_params(
        &oxy_airlayer_compat::request_dialect(&eng, request),
        &compiled.sql,
        &compiled.params,
    )
}

/// Assert both paths answer `request` identically, and that the rollup path
/// was actually taken — an equivalence that passes because the rollup silently
/// did not apply is the failure this whole surface is about.
fn assert_paths_agree(request: QueryRequest, what: &str, expected_rows: usize) {
    let workspace = ScratchWorkspace::new();
    let conn = fixture(&workspace);

    // Through the shipped resolver, not a parallel call to airlayer — so this
    // covers the coverage check, the source choice and the SQL generation the
    // product actually runs.
    let compiled = try_resolve_preagg(
        &preagg_ctx(&workspace, None),
        &request,
        "SELECT 'unused'",
        "local",
        None,
    )
    .unwrap_or_else(|| panic!("{what}: no rollup covered the request"));
    let CompiledQuery::Preaggregation {
        preagg_sql: reagg_sql,
        source,
        ..
    } = compiled
    else {
        panic!("{what}: expected a rollup resolution");
    };
    assert!(!source.is_remote(), "{what}: this case is the local tier");

    let from_warehouse = rows(&conn, &warehouse_sql(&request));
    let from_rollup = rows(&conn, &reagg_sql);

    // Pinning the row COUNT as well as the values is what keeps this test
    // honest: a request whose time dimension silently failed to parse would
    // still compare equal — both sides would just answer a coarser question —
    // and the exact-grain case, the one that skips re-aggregation entirely,
    // would never actually be exercised.
    assert_eq!(
        from_warehouse.len(),
        expected_rows,
        "{what}: the warehouse answered {} rows, not the {expected_rows} this case is about",
        from_warehouse.len()
    );
    assert_eq!(
        from_rollup, from_warehouse,
        "{what}: the rollup answered differently than the warehouse\n\
         re-agg SQL:\n{reagg_sql}"
    );
}

/// Exact grain: the request asks for exactly what the rollup stores. This is
/// the case that skips `GROUP BY` entirely, so nothing re-aggregates and the
/// stored rows are returned as-is — which is only correct if they already are
/// the answer.
#[test]
fn a_query_at_the_rollup_s_own_grain_matches_the_warehouse() {
    let request: QueryRequest = serde_json::from_value(serde_json::json!({
        "measures": [
            "orders.total_orders",
            "orders.total_amount",
            "orders.avg_amount",
            "orders.distinct_customers"
        ],
        "dimensions": ["orders.status"],
        "time_dimensions": [{"dimension": "orders.order_date", "granularity": "month"}]
    }))
    .expect("request parses");
    // 2 months x 2 statuses.
    assert_paths_agree(request, "exact grain", 4);
}

/// Coarser grain: no time dimension at all, so the rollup's month rows must be
/// folded together. `avg` and `count_distinct` are where a naive fold breaks —
/// the average of two monthly averages is not the overall average, and summing
/// two monthly distinct-counts double-counts a customer who ordered in both.
#[test]
fn a_query_coarser_than_the_rollup_matches_the_warehouse() {
    let request: QueryRequest = serde_json::from_value(serde_json::json!({
        "measures": [
            "orders.total_orders",
            "orders.total_amount",
            "orders.avg_amount",
            "orders.distinct_customers"
        ],
        "dimensions": ["orders.status"]
    }))
    .expect("request parses");
    // 2 statuses, both months folded together.
    assert_paths_agree(request, "coarser grain (no time dimension)", 2);
}

/// The regression `airlayer#99` fixed, pinned here because it is the exact
/// shape the Pre-aggregated badge is a promise against: `Measure.filters` never
/// reached the rollup's CTAS, so `paid_amount` STORED the unfiltered total and
/// served it under the badge. Grouping by `status` makes it unmissable — the
/// `pending` rows must read 0, and the buggy build made them equal
/// `total_amount`.
///
/// The filter's `{{status}}` reference covers the other half of that fix. That
/// half was loud rather than silent (an unresolved `{{` reached the warehouse
/// as a parser error, so nothing wrong was ever cached), but a rollup that
/// cannot build is a rollup nobody notices is missing.
///
/// It is also the regression test for the SHAPE problem behind
/// `PREAGG_BUILDER_GENERATION`: a change to what a rollup stores is invisible
/// to the rollup hash, so an equivalence test is the only thing that catches
/// the next one at build time rather than in a customer's dashboard.
#[test]
fn a_filtered_measure_matches_the_warehouse() {
    let request: QueryRequest = serde_json::from_value(serde_json::json!({
        "measures": ["orders.total_amount", "orders.paid_amount"],
        "dimensions": ["orders.status"],
        "time_dimensions": [{"dimension": "orders.order_date", "granularity": "month"}]
    }))
    .expect("request parses");
    assert_paths_agree(request, "filtered measure at exact grain", 4);

    // Folding months together re-aggregates the stored partial, so a filter
    // that was applied at build time has to survive the fold as well.
    let request: QueryRequest = serde_json::from_value(serde_json::json!({
        "measures": ["orders.total_amount", "orders.paid_amount"],
        "dimensions": ["orders.status"]
    }))
    .expect("request parses");
    assert_paths_agree(request, "filtered measure, months folded", 2);
}

/// Dropping a dimension folds across `status` as well, so both the time and
/// the categorical axis re-aggregate at once.
#[test]
fn a_query_dropping_a_dimension_matches_the_warehouse() {
    let request: QueryRequest = serde_json::from_value(serde_json::json!({
        "measures": ["orders.total_orders", "orders.total_amount"],
        "dimensions": [],
        "time_dimensions": [{"dimension": "orders.order_date", "granularity": "month"}]
    }))
    .expect("request parses");
    // 2 months, both statuses folded together.
    assert_paths_agree(request, "dimension dropped", 2);
}

/// With the Parquet gone and no blob store configured, the resolution must
/// decline rather than hand back SQL pointing at a file that isn't there.
/// A missing rollup is a slower answer, never a wrong one and never an error.
#[test]
fn a_missing_parquet_and_no_blob_store_declines_instead_of_answering() {
    let workspace = ScratchWorkspace::new();
    let _conn = fixture(&workspace);
    drop_local_parquets(&workspace);

    let request: QueryRequest = serde_json::from_value(serde_json::json!({
        "measures": ["orders.total_amount"],
        "dimensions": ["orders.status"]
    }))
    .expect("request parses");
    assert!(
        try_resolve_preagg(
            &preagg_ctx(&workspace, None),
            &request,
            "SELECT 'unused'",
            "local",
            None,
        )
        .is_none(),
        "a manifest entry whose Parquet is absent, with nowhere else to read it, must not resolve"
    );
}

/// A blob source that points at nothing must FAIL, loudly, at execute time —
/// so the caller can fall back to the warehouse.
///
/// This is the contract the three execute sites rely on
/// (`api::semantic::execute_semantic_query`, the analytics executing handler,
/// the builder's `semantic_query` tool). It matters that this is an `Err` and
/// not an empty result set: empty rows are indistinguishable from a rollup
/// that genuinely has none, and would be served to the user as an answer.
#[test]
fn a_blob_source_pointing_at_nothing_errors_rather_than_answering_empty() {
    let workspace = ScratchWorkspace::new();
    let _conn = fixture(&workspace);
    drop_local_parquets(&workspace);

    let request: QueryRequest = serde_json::from_value(serde_json::json!({
        "measures": ["orders.total_amount"],
        "dimensions": ["orders.status"]
    }))
    .expect("request parses");

    let blob = BlobConfig {
        // A bucket that does not exist, reached through an endpoint that does
        // not answer — the shape of a manifest listing a rollup whose object
        // was never mirrored.
        bucket: "oxy-nonexistent-bucket-for-tests".to_string(),
        region: Some("us-east-1".to_string()),
        endpoint_url: Some("http://127.0.0.1:1".to_string()),
    };
    let compiled = try_resolve_preagg(
        &preagg_ctx(&workspace, Some(blob)),
        &request,
        "SELECT 'unused'",
        "local",
        None,
    )
    .expect("the blob tier resolves without checking the object exists");
    let CompiledQuery::Preaggregation {
        preagg_sql, source, ..
    } = compiled
    else {
        panic!("expected a rollup resolution");
    };

    let result = crate::preagg::execute_preagg_sql(&preagg_sql, &source);
    assert!(
        result.is_err(),
        "an unreadable rollup must report, not answer: {result:?}"
    );
}

/// The fleet case, and the reason the blob tier exists: with the Parquet gone
/// but a blob store configured, the SAME re-aggregation runs — only the FROM
/// clause moves.
///
/// This is the equivalence that matters for the badge. The two tiers cannot
/// answer differently, because the only difference between them is the string
/// inside `read_parquet(...)`; asserting that directly is stronger than
/// running the remote one, which would need a live S3.
#[test]
fn the_blob_tier_generates_the_same_sql_with_only_the_source_swapped() {
    let workspace = ScratchWorkspace::new();
    let _conn = fixture(&workspace);

    let request: QueryRequest = serde_json::from_value(serde_json::json!({
        "measures": ["orders.total_orders", "orders.avg_amount"],
        "dimensions": ["orders.status"]
    }))
    .expect("request parses");

    let local = try_resolve_preagg(
        &preagg_ctx(&workspace, None),
        &request,
        "SELECT 'unused'",
        "local",
        None,
    )
    .expect("local tier resolves");
    let CompiledQuery::Preaggregation {
        preagg_sql: local_sql,
        source: local_source,
        ..
    } = local
    else {
        panic!("expected a rollup resolution");
    };
    let local_path = local_source
        .local_path()
        .expect("the local tier reads a file")
        .to_string_lossy()
        .to_string();

    // Same workspace, same manifest — only this node's copy of the Parquet is
    // gone, which is every node but the builder.
    drop_local_parquets(&workspace);
    let blob = BlobConfig {
        bucket: "oxy-blobs".to_string(),
        region: Some("us-east-1".to_string()),
        endpoint_url: None,
    };
    let remote = try_resolve_preagg(
        &preagg_ctx(&workspace, Some(blob)),
        &request,
        "SELECT 'unused'",
        "local",
        None,
    )
    .expect("blob tier resolves");
    let CompiledQuery::Preaggregation {
        preagg_sql: remote_sql,
        source: remote_source,
        ..
    } = remote
    else {
        panic!("expected a rollup resolution");
    };
    assert!(remote_source.is_remote());
    let remote_uri = match &remote_source {
        PreaggSource::Blob { uri, .. } => uri.clone(),
        other => panic!("expected a blob source, got {other:?}"),
    };
    assert!(
        remote_uri.starts_with(&format!("s3://oxy-blobs/runtime/preagg/{}/", workspace.id)),
        "unexpected object URI: {remote_uri}"
    );

    // The projection, grouping and re-aggregation are byte-identical; swap the
    // source back and the two SQL strings are the same string.
    assert_eq!(
        remote_sql.replace(&remote_uri, &local_path),
        local_sql,
        "the two tiers must differ only in the source they read"
    );

    // And nothing was downloaded to make the remote tier work.
    assert!(
        std::fs::read_dir(&workspace.dir)
            .expect("cache dir reads")
            .filter_map(Result::ok)
            .all(|e| e.path().extension().is_none_or(|x| x != "parquet")),
        "the blob tier must not write a local copy"
    );
}
