//! End-to-end scenario fit against `example_new`'s shipped fixture.
//!
//! Everything else about the metric-tree fit is unit-tested against fake
//! executors, which is why two separate query-shape bugs reached the UI: a
//! fake executor answers whatever it is told to, so neither the additive/
//! non-additive refusal nor the split's row merge was ever exercised against
//! real SQL. This runs the real path — compile the panel query, execute it on
//! the committed CSVs through DuckDB, fit — so a regression shows up here
//! rather than as a refusal on someone's screen.
//!
//! Every test here `set_current_dir`s — the view SQL reads its CSVs by
//! relative path, and cwd is process-global — so every one is
//! `#[serial_test::serial]`.
//!
//! That attribute, not a lock private to this file, is what makes the cd safe.
//! This module used to be its own `tests/*.rs` binary, where a private mutex
//! was enough because nothing else in the process touched cwd. It is now a
//! `mod` in the `platform` binary, which already contains a second cwd mutator
//! (`workspace_details_fields::storage_key_normalizes_relative_to_absolute`),
//! and a private mutex excludes nothing across module boundaries.
//! `serial_test`'s lock is global to the binary, so the two coordinate — which
//! is why that test already uses it.
//!
//! `cargo nextest`, which this repo mandates, gives each test its own process
//! and would not need any of this. The attribute is what a stranger typing
//! `cargo test` gets instead of a race.

use std::path::PathBuf;

use oxy_airlayer_compat::engine::EngineError;
use oxy_airlayer_compat::engine::metric_tree_fit::{
    fit_driver_coefficients, fit_panel_dimensions, fittable_edges,
};
use oxy_airlayer_compat::engine::metric_tree_ops::QueryExecutor;
use oxy_airlayer_compat::engine::query::{FilterOperator, QueryFilter, QueryRequest};
use oxy_airlayer_compat::{DatasourceDialectMap, SemanticEngine, SemanticLayer};

/// The window the scenario panel defaults to, pinned rather than derived from
/// today: the fixture stops at 2026-07-19, so a window computed from the clock
/// silently empties out as time passes and the test would start asserting
/// nothing.
const WINDOW: (&str, &str) = ("2026-05-08", "2026-08-05");

/// `cd` into the fixture's data directory.
///
/// Call this ONLY from a `#[serial_test::serial]` test: that attribute holds a
/// binary-global lock for the test's whole body, which is what makes the cd
/// and the CSV reads that depend on it atomic against the rest of the
/// `platform` binary. A lock private to this file could not do that, and would
/// be worse than none because it would read as protection.
/// A deadline far enough out that these tests never trip it — they are here to
/// assert query SHAPES against real SQL, not the split's time budget, which is
/// unit-tested in `metric_tree_baseline`.
fn no_deadline() -> std::time::Instant {
    std::time::Instant::now() + std::time::Duration::from_secs(3600)
}

pub(crate) fn enter_fixture_dir() {
    std::env::set_current_dir(repo_root().join("example_new/.db")).unwrap();
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

pub(crate) fn layer() -> SemanticLayer {
    let dir = repo_root().join("example_new/semantics/views");
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    paths.sort();
    let views = paths
        .iter()
        .map(|p| {
            oxy_airlayer_compat::parse_view_yaml(&std::fs::read_to_string(p).unwrap()).unwrap()
        })
        .collect();
    SemanticLayer::new(views, None)
}

fn json_of(v: duckdb::types::Value) -> serde_json::Value {
    use duckdb::types::Value as V;
    match v {
        V::Null => serde_json::Value::Null,
        V::Boolean(b) => serde_json::json!(b),
        V::TinyInt(i) => serde_json::json!(i),
        V::SmallInt(i) => serde_json::json!(i),
        V::Int(i) => serde_json::json!(i),
        V::BigInt(i) => serde_json::json!(i),
        // DuckDB widens a SUM over integers to HUGEINT. Dropping this arm
        // turns every summed measure into a debug string, which reads as a
        // present-but-unparseable column rather than a missing one.
        // Through `i64` where it fits, so a value inside JSON's exact integer
        // range keeps every digit; only a genuinely huge one takes the lossy
        // `f64` path, and then it is already past what a JSON number can hold.
        V::HugeInt(i) => match i64::try_from(i) {
            Ok(n) => serde_json::json!(n),
            Err(_) => serde_json::json!(i as f64),
        },
        V::UTinyInt(i) => serde_json::json!(i),
        V::USmallInt(i) => serde_json::json!(i),
        V::UInt(i) => serde_json::json!(i),
        V::UBigInt(i) => serde_json::json!(i),
        V::Float(f) => serde_json::json!(f),
        V::Double(f) => serde_json::json!(f),
        V::Decimal(d) => serde_json::json!(d.to_string()),
        V::Text(s) => serde_json::json!(s),
        V::Date32(d) => {
            let base = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
            serde_json::json!((base + chrono::Duration::days(d as i64)).to_string())
        }
        // `date_trunc('day', …)` comes back as a TIMESTAMP; production's typed
        // stream renders it as ISO text, so the fixture does the same.
        V::Timestamp(unit, v) => {
            let micros = match unit {
                duckdb::types::TimeUnit::Second => v.saturating_mul(1_000_000),
                duckdb::types::TimeUnit::Millisecond => v.saturating_mul(1_000),
                duckdb::types::TimeUnit::Microsecond => v,
                duckdb::types::TimeUnit::Nanosecond => v / 1_000,
            };
            match chrono::DateTime::from_timestamp_micros(micros) {
                Some(t) => serde_json::json!(t.naive_utc().format("%Y-%m-%dT%H:%M:%S").to_string()),
                None => serde_json::json!(format!("{unit:?} {v}")),
            }
        }
        other => serde_json::json!(format!("{other:?}")),
    }
}

/// Compiles each request and runs it on the fixture CSVs.
pub(crate) fn duckdb_executor(engine: SemanticEngine) -> Box<QueryExecutor> {
    Box::new(move |req: &QueryRequest| {
        let compiled = engine.compile_query(req)?;
        let conn = duckdb::Connection::open_in_memory()
            .map_err(|e| EngineError::QueryError(e.to_string()))?;
        let mut stmt = conn
            .prepare(&compiled.sql)
            .map_err(|e| EngineError::QueryError(e.to_string()))?;
        let mut rows = stmt
            .query(duckdb::params_from_iter(compiled.params.iter()))
            .map_err(|e| EngineError::QueryError(e.to_string()))?;
        // Aliases off the statement, not off `compiled.columns`: a mismatch
        // between airlayer's declared order and the warehouse's would
        // otherwise relabel every value and the assertions below would be
        // checking the wrong columns.
        let names: Vec<String> = rows
            .as_ref()
            .map(|s| s.column_names())
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| EngineError::QueryError(e.to_string()))?
        {
            let mut map = serde_json::Map::new();
            for (i, name) in names.iter().enumerate() {
                let v: duckdb::types::Value = row.get(i).unwrap_or(duckdb::types::Value::Null);
                map.insert(name.clone(), json_of(v));
            }
            out.push(map);
        }
        Ok(out)
    })
}

fn date_filter(op: FilterOperator, value: &str) -> QueryFilter {
    QueryFilter {
        member: Some("checks.check_date".to_string()),
        operator: Some(op),
        values: vec![value.to_string()],
        and: None,
        or: None,
    }
}

/// Per-panel spread of one measure across the rows: how many panels have ≥2
/// rows, and in how many of those the measure never moves. A measure that is
/// flat in every panel is exactly what the fit reports as "the driver does not
/// vary within any panel".
fn within_panel(
    rows: &[serde_json::Map<String, serde_json::Value>],
    alias: &str,
) -> (usize, usize, f64) {
    let mut by_panel: std::collections::HashMap<String, Vec<f64>> = Default::default();
    for r in rows {
        let key = format!(
            "{:?}|{:?}",
            r.get("checks__location_id"),
            r.get("checks__server_id")
        );
        if let Some(v) = r.get(alias).and_then(|v| v.as_f64()) {
            by_panel.entry(key).or_default().push(v);
        }
    }
    let multi: Vec<_> = by_panel.values().filter(|v| v.len() >= 2).collect();
    let flat = multi
        .iter()
        .filter(|v| v.iter().all(|x| (x - v[0]).abs() < 1e-9))
        .count();
    let all: Vec<f64> = multi.iter().flat_map(|v| v.iter().copied()).collect();
    let mean = all.iter().sum::<f64>() / all.len().max(1) as f64;
    (multi.len(), flat, mean)
}

/// Pairing a check-grain sum with a store-grain sum in one request joins the
/// coarse view in on `location_id` alone and fans BOTH measures out. This is
/// the shape an additivity-only split used to send, and it is why the fit
/// reported "the driver does not vary within any panel" against data that
/// varies in every one of them.
///
/// Asserted against real SQL because no fake executor can produce a join
/// fan-out — which is exactly why this reached the UI instead of a test.
#[test]
#[serial_test::serial]
fn a_check_grain_sum_is_corrupted_by_sharing_a_request_with_a_store_grain_one() {
    enter_fixture_dir();
    let layer = layer();
    let dialects = DatasourceDialectMap::with_default(oxy_airlayer_compat::Dialect::DuckDB);
    let engine = SemanticEngine::from_semantic_layer(layer.clone(), dialects).unwrap();
    let raw = duckdb_executor(engine);

    let ask = |measures: &[&str]| {
        raw(&QueryRequest {
            measures: measures.iter().map(|m| m.to_string()).collect(),
            dimensions: vec![
                "checks.location_id".into(),
                "checks.server_id".into(),
                "checks.check_date".into(),
            ],
            filters: vec![
                date_filter(FilterOperator::AfterOrOnDate, WINDOW.0),
                date_filter(FilterOperator::BeforeOrOnDate, WINDOW.1),
            ],
            limit: Some(oxy_airlayer_compat::engine::UNBOUNDED_QUERY_LIMIT),
            ..QueryRequest::new()
        })
        .expect("query runs")
    };

    // Alone — the shape the split must send — guests are the real aggregate
    // and move from day to day in every panel.
    let (panels, flat, mean) = within_panel(&ask(&["checks.total_guests"]), "checks__total_guests");
    assert_eq!(panels, 360);
    assert_eq!(flat, 0, "guests must vary within every panel");
    assert!((mean - 3.886).abs() < 0.01, "guests per server-day: {mean}");

    // Shared with a store-grain sum, the same measure is inflated and pinned
    // flat. Pinned as a known engine behaviour, not as something desirable:
    // the split's job is never to send this.
    let (panels, flat, mean) = within_panel(
        &ask(&["checks.total_guests", "store_days.net_sales"]),
        "checks__total_guests",
    );
    assert_eq!(panels, 360);
    assert_eq!(
        flat, 360,
        "the cross-grain join flattens guests in every panel"
    );
    assert!(mean > 1000.0, "and inflates them: {mean}");
}

/// Wrap an executor so it refuses a request that mixes additive and
/// non-additive measures from one view, the way airlayer's fan-out guard does
/// when a scope or dimension drags a one-to-many join into that view.
///
/// This is the condition that makes the split run at all, and the reason the
/// corruption above reached a user rather than a test: against a plain DuckDB
/// executor the guard never fires, the split never runs, and everything looks
/// fine. Forcing it here is what tests the path production is actually on.
fn guard_like_airlayer(layer: SemanticLayer, inner: Box<QueryExecutor>) -> Box<QueryExecutor> {
    Box::new(move |req: &QueryRequest| {
        let mut by_view: std::collections::HashMap<&str, (bool, bool)> = Default::default();
        for m in &req.measures {
            let (view, name) = m.split_once('.').unwrap_or((m.as_str(), ""));
            let additive = layer
                .views
                .iter()
                .find(|v| v.name == view)
                .and_then(|v| {
                    v.measures_list()
                        .iter()
                        .find(|x| x.name == name)
                        .map(|x| x.measure_type.clone())
                })
                .is_some_and(|t| {
                    use oxy_airlayer_compat::schema::models::MeasureType as M;
                    matches!(t, M::Sum | M::Count | M::Min | M::Max)
                });
            let e = by_view.entry(view).or_insert((false, false));
            if additive { e.0 = true } else { e.1 = true }
        }
        if by_view.values().any(|(a, n)| *a && *n) {
            return Err(EngineError::QueryError(
                "Cannot combine additive and non-additive measures from view '?' in one query"
                    .to_string(),
            ));
        }
        inner(req)
    })
}

/// The production path: the guard refuses the batch, the split retries it, and
/// the fit runs on what the split merged back. This is where an
/// additivity-only split silently handed the fit a flattened driver.
#[test]
#[serial_test::serial]
fn the_fit_survives_a_batch_the_guard_refuses() {
    enter_fixture_dir();
    let layer = layer();
    let tree = oxy_semantic::build_metric_tree(&layer);
    let roots = vec!["checks.total_guests".to_string()];
    let panel_dimensions = fit_panel_dimensions(&layer, &fittable_edges(&tree, &roots));

    let dialects = DatasourceDialectMap::with_default(oxy_airlayer_compat::Dialect::DuckDB);
    let engine = SemanticEngine::from_semantic_layer(layer.clone(), dialects).unwrap();
    let guarded = guard_like_airlayer(layer.clone(), duckdb_executor(engine));
    let executor = oxy_app::server::api::metric_tree_baseline::splitting_executor(
        layer,
        guarded,
        no_deadline(),
    );

    let fits = fit_driver_coefficients(
        &tree,
        &roots,
        &panel_dimensions,
        "checks.check_date",
        WINDOW,
        &[],
        &*executor,
    )
    .expect("the fit runs");

    // Every edge sees the full panel, split or not.
    for f in &fits {
        assert!(
            f.n > 10_000 && f.n_panels == 360,
            "{} -> {}: n={} panels={}",
            f.from,
            f.to,
            f.n,
            f.n_panels
        );
    }

    // Both check-grain edges get a real magnitude. These are the ones the
    // additivity-only split silently flattened, and `total_guests` is the one
    // that made a pinned "Guests served" lever move nothing at all.
    for from in ["checks.total_guests", "checks.net_revenue"] {
        let f = fits
            .iter()
            .find(|f| f.from == from && f.to.starts_with("checks."))
            .unwrap_or_else(|| panic!("{from} has no check-grain fit"));
        assert!(f.refusal.is_none(), "{from} refused: {:?}", f.refusal);
        assert!(
            f.coefficient.is_some_and(|c| c.is_finite() && c > 0.0),
            "{from} bought nothing: {f:?}"
        );
    }

    // The cross-grain edge still cannot be sized, and that is the honest
    // outcome rather than a regression: `store_days.net_sales` under
    // check-grain dimensions joins on `location_id` alone, so it arrives as a
    // per-location constant and the fit measures t = 0. Asserted as a REFUSAL
    // — the one thing that must never happen here is a confident number
    // derived from a fanned-out column. Fixing the join is what would let this
    // edge size; until then this is the guard against pretending it did.
    let cross_grain = fits
        .iter()
        .find(|f| f.to == "store_days.net_sales")
        .expect("the grain-bridge edge is a candidate");
    assert!(
        cross_grain.refusal.is_some() && cross_grain.coefficient.is_none(),
        "a fanned-out cross-grain measure must refuse, not report a magnitude: {cross_grain:?}"
    );
}

#[test]
#[serial_test::serial]
fn the_scenario_fit_sizes_every_driver_edge_against_the_shipped_fixture() {
    enter_fixture_dir();

    let layer = layer();
    let tree = oxy_semantic::build_metric_tree(&layer);
    // `checks.total_guests` is the lever whose reachable set spans both grains
    // AND mixes additivity, so it is the one that exercised both refusals.
    let roots = vec!["checks.total_guests".to_string()];

    let candidates = fittable_edges(&tree, &roots);
    let panel_dimensions = fit_panel_dimensions(&layer, &candidates);
    assert_eq!(
        panel_dimensions,
        vec!["checks.location_id", "checks.server_id"],
        "the panel is (location, server); a change here rescales every fit"
    );

    let dialects = DatasourceDialectMap::with_default(oxy_airlayer_compat::Dialect::DuckDB);
    let engine = SemanticEngine::from_semantic_layer(layer.clone(), dialects).unwrap();
    let executor = oxy_app::server::api::metric_tree_baseline::splitting_executor(
        layer,
        duckdb_executor(engine),
        no_deadline(),
    );

    // The panel query itself: a mixed batch that only runs because the
    // executor splits it. Before the split this was the whole failure.
    let probe = QueryRequest {
        measures: vec![
            "checks.total_guests".into(),
            "checks.net_revenue".into(),
            "checks.gross_profit".into(),
        ],
        dimensions: vec![
            "checks.location_id".into(),
            "checks.server_id".into(),
            "checks.check_date".into(),
        ],
        filters: vec![
            date_filter(FilterOperator::AfterOrOnDate, WINDOW.0),
            date_filter(FilterOperator::BeforeOrOnDate, WINDOW.1),
        ],
        limit: Some(oxy_airlayer_compat::engine::UNBOUNDED_QUERY_LIMIT),
        ..QueryRequest::new()
    };
    let rows = executor(&probe).expect("a mixed panel batch must survive the split");
    assert_eq!(
        rows.len(),
        13_498,
        "one row per (location, server, day) in the window"
    );

    // Every row carries BOTH halves' measures. A merge that stacked the halves
    // instead of joining them would leave each row with one or the other, and
    // the fit would then see no paired observations at all.
    for row in &rows {
        for alias in [
            "checks__total_guests",
            "checks__net_revenue",
            "checks__gross_profit",
        ] {
            assert!(row.contains_key(alias), "row missing {alias}: {row:?}");
        }
    }

    // And the values are the real aggregates, not fanned-out multiples of
    // them: means computed straight off the CSVs are 3.886 guests and $203.47
    // per server-day.
    let mean = |alias: &str| {
        let vals: Vec<f64> = rows.iter().filter_map(|r| r.get(alias)?.as_f64()).collect();
        vals.iter().sum::<f64>() / vals.len() as f64
    };
    assert!(
        (mean("checks__total_guests") - 3.886).abs() < 0.01,
        "guests per server-day drifted: {}",
        mean("checks__total_guests")
    );
    assert!(
        (mean("checks__net_revenue") - 203.47).abs() < 0.1,
        "revenue per server-day drifted: {}",
        mean("checks__net_revenue")
    );

    let fits = fit_driver_coefficients(
        &tree,
        &roots,
        &panel_dimensions,
        "checks.check_date",
        WINDOW,
        &[],
        &*executor,
    )
    .expect("the fit runs");

    assert_eq!(fits.len(), 3, "three fittable driver edges: {fits:?}");
    // The panel shape is asserted for EVERY edge, refused or not: a t-gate
    // refusal still reports the fit context it was measured on, so a collapse
    // here means the query shape regressed rather than that an edge declined.
    // The data supports far more than the 30-observation floor, so a collapse
    // to a handful means the query shape regressed, not that the fixture got
    // thin.
    for f in &fits {
        assert!(
            f.n > 10_000 && f.n_panels == 360,
            "{} -> {} fitted on n={} panels={} — the panel collapsed",
            f.from,
            f.to,
            f.n,
            f.n_panels
        );
    }
    // A magnitude is demanded only of the check-grain edges. The cross-grain
    // one is pinned as a refusal below — demanding a coefficient from it here
    // would be demanding a number off a provably constant column.
    for f in fits.iter().filter(|f| f.to.starts_with("checks.")) {
        assert!(
            f.refusal.is_none(),
            "{} -> {} was refused: {:?}",
            f.from,
            f.to,
            f.refusal
        );
        assert!(
            f.coefficient.is_some(),
            "{} -> {} produced no coefficient",
            f.from,
            f.to
        );
    }

    // The fit that the whole scenario hangs on, through the split, end to end.
    // `n` alone is not enough: the split's first version produced n=13,498 and
    // a refusal, because the rows it merged had been corrupted on the way in.
    let guests = fits
        .iter()
        .find(|f| f.from == "checks.total_guests")
        .expect("the guests -> revenue edge is fitted");
    assert!(
        guests.coefficient.is_some_and(|c| c.is_finite() && c > 0.0),
        "guests must buy revenue, not nothing: {guests:?}"
    );

    // The grain bridge, asserted as a REFUSAL. Split by (view, additivity),
    // the `store_days` half runs alone under check-grain dimensions and joins
    // into the spine on `location_id` with no date predicate — so every
    // store-day's sales are summed into every row and `net_sales` arrives as
    // one whole-window total per location, constant in all 360 panels. Zero
    // within-panel variance measures as t = 0.00 and is refused.
    //
    // Before the split grouped by view as well as additivity, this edge came
    // back with an elasticity of 0.42 for what the model calls an identity —
    // a confident number off a fanned-out column. That is the failure this
    // pins against; fixing the dateless join is what would let the edge size.
    let cross_grain = fits
        .iter()
        .find(|f| f.to == "store_days.net_sales")
        .expect("the grain-bridge edge is a candidate");
    assert!(
        cross_grain.refusal.is_some() && cross_grain.coefficient.is_none(),
        "a fanned-out cross-grain measure must refuse, not report a magnitude: {cross_grain:?}"
    );
}

/// Grouping check-grain measures by a STORE-grain time dimension joins
/// `store_days` in on `location_id` alone — nothing ties a check's date to a
/// store-day — so every check of a server joins to all 73 store-days of its
/// location. Each cell holds that server's whole-window total, identical on
/// every date, and the fit refuses on a driver that "does not vary within any
/// panel".
///
/// This is what a user actually hit, and the numbers below are the ones that
/// were on their screen. The picker now only offers dimensions on a pinned
/// lever's own view (`usableTimeDimensions`); this is the engine-side proof of
/// why that restriction exists, so nobody relaxes it back.
#[test]
#[serial_test::serial]
fn a_foreign_grain_time_dimension_flattens_every_measure() {
    enter_fixture_dir();
    let layer = layer();
    let dialects = DatasourceDialectMap::with_default(oxy_airlayer_compat::Dialect::DuckDB);
    let engine = SemanticEngine::from_semantic_layer(layer.clone(), dialects).unwrap();
    let split = oxy_app::server::api::metric_tree_baseline::splitting_executor(
        layer.clone(),
        duckdb_executor(engine),
        no_deadline(),
    );

    let ask = |td: &str| {
        split(&QueryRequest {
            measures: vec![
                "checks.total_guests".into(),
                "checks.net_revenue".into(),
                "checks.gross_profit".into(),
            ],
            dimensions: vec![
                "checks.location_id".into(),
                "checks.server_id".into(),
                td.into(),
            ],
            filters: vec![
                QueryFilter {
                    member: Some(td.into()),
                    operator: Some(FilterOperator::AfterOrOnDate),
                    values: vec![WINDOW.0.into()],
                    and: None,
                    or: None,
                },
                QueryFilter {
                    member: Some(td.into()),
                    operator: Some(FilterOperator::BeforeOrOnDate),
                    values: vec![WINDOW.1.into()],
                    and: None,
                    or: None,
                },
            ],
            limit: Some(oxy_airlayer_compat::engine::UNBOUNDED_QUERY_LIMIT),
            ..QueryRequest::new()
        })
        .expect("query runs")
    };

    // The lever's own view: a ragged panel of the days each server actually
    // worked, and every measure moves.
    let rows = ask("checks.check_date");
    let (panels, flat, mean) = within_panel(&rows, "checks__total_guests");
    assert_eq!((rows.len(), panels, flat), (13_498, 360, 0));
    assert!((mean - 3.886).abs() < 0.01, "guests: {mean}");

    // A store-grain date: a perfect 360 x 73 rectangle, every measure frozen.
    // 26,280 observations that are one value repeated 73 times per panel — an
    // `n` large enough to sail past every sample-size gate the fit has.
    let rows = ask("store_days.business_date");
    assert_eq!(rows.len(), 26_280, "360 panels x 73 store-days");
    for alias in [
        "checks__total_guests",
        "checks__net_revenue",
        "checks__gross_profit",
    ] {
        let (panels, flat, _) = within_panel(&rows, alias);
        assert_eq!(
            (panels, flat),
            (360, 360),
            "{alias} must be flat in every panel under a foreign grain"
        );
    }
}
