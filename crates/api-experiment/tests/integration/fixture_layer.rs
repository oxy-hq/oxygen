//! The `example_new` fixture as a semantic layer, and a DuckDB executor over its
//! committed CSVs — what `experiment_pricing` runs the real price job against.
//!
//! Copied from `oxy-app`'s `tests/platform/metric_tree_fit_panel.rs` when the
//! experiment tests moved here with their code: `oxy-app` cannot depend on this
//! crate, and a test binary cannot share a module with another crate's. The
//! metric-tree fit tests keep their copy; if the fixture's shape changes, both
//! change.
//!
//! `enter_fixture_dir` `set_current_dir`s, and cwd is process-global, so every
//! test that calls it is `#[serial_test::serial]` — that attribute's lock is
//! global to this binary, which is what makes the cd and the CSV reads that
//! depend on it atomic against the rest of the group.

use std::path::PathBuf;

use oxy_airlayer_compat::engine::EngineError;
use oxy_airlayer_compat::engine::metric_tree_ops::QueryExecutor;
use oxy_airlayer_compat::engine::query::QueryRequest;
use oxy_airlayer_compat::{SemanticEngine, SemanticLayer};

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
