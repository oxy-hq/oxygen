//! Every `BaselineOutcome` a values read can return, and what the run does
//! about it — plus one read through the real engine, for what only it can show.
//!
//! Asserted here rather than through `FitProbe::probe` because the thing worth
//! pinning is the *decision*, not the query: the bug this file exists for was
//! a discarded outcome, and a test that needed a warehouse to catch it would
//! never have run in CI. `classify_values` is pure, so every variant —
//! including the ones no executor emits today — is one line.

use oxy_airlayer_compat::engine::metric_tree_ops::BaselineOutcome as O;

use super::{FitProbe, ValuesVerdict, classify_values};

/// The ordinary case, and the only one that is silent.
#[test]
fn a_clean_read_is_complete() {
    assert_eq!(
        classify_values(&O::Valued {
            unreadable: Vec::new()
        }),
        ValuesVerdict::Complete,
    );
}

/// The regression this module was written for.
///
/// Before the fix the outcome was dropped, so an executor error left `values`
/// empty, every `EdgeFit` unlevelled, and `Outcome::classify` scored the
/// period `Refused` with `refusal` NULL — a broken read wearing the label the
/// taxonomy reserves for the model honestly declining. It must fail the run.
#[test]
fn an_executor_error_fails_the_run_rather_than_refusing() {
    let verdict = classify_values(&O::ExecutorError("connection reset".to_string()));

    match verdict {
        ValuesVerdict::Fatal(what) => assert!(
            what.contains("connection reset"),
            "the fatal message must carry the warehouse's own error, got: {what}",
        ),
        other => panic!("an executor error must be fatal, got {other:?}"),
    }
}

/// A partial read still measures something, so it warns rather than failing —
/// but it must not be mistaken for a clean one, or the warn never fires.
#[test]
fn a_partial_read_is_degraded_and_names_the_measures() {
    let verdict = classify_values(&O::Valued {
        unreadable: vec!["store_day.net_sales".to_string()],
    });

    match verdict {
        ValuesVerdict::Degraded(what) => assert!(
            what.contains("store_day.net_sales"),
            "a degraded read must name what was lost, got: {what}",
        ),
        other => panic!("a partial read must be degraded, got {other:?}"),
    }
}

/// An empty window is a legitimate state for an early period, so it degrades
/// rather than failing — but it is still not `Complete`.
#[test]
fn the_non_fatal_empty_shapes_all_degrade() {
    for outcome in [
        O::NoRows,
        O::NoMatchingColumns,
        O::NothingRequested,
        O::UnreadableValues(vec!["store_day.marketing_spend".to_string()]),
    ] {
        assert!(
            matches!(classify_values(&outcome), ValuesVerdict::Degraded(_)),
            "{outcome:?} must degrade, not pass silently or fail the run",
        );
    }
}

/// `NoRows` and `NothingRequested` are different diagnoses — an empty window
/// versus an empty tree — and must not collapse into one message, or the warn
/// sends someone to widen a window that was never the problem.
#[test]
fn an_empty_window_and_an_empty_tree_read_differently() {
    let (ValuesVerdict::Degraded(no_rows), ValuesVerdict::Degraded(nothing)) = (
        classify_values(&O::NoRows),
        classify_values(&O::NothingRequested),
    ) else {
        panic!("both must be degraded");
    };

    assert_ne!(
        no_rows, nothing,
        "an empty window and an unreachable root must not share a message",
    );
}

/// Stores, and the sales that induce `net_sales` up onto them through the
/// shared `store_id` entity. `stores.sales_per_store` reads that induced name,
/// which no tree node carries.
const STORES_VIEW: &str = r#"
name: stores
datasource: sim
table: stores
entities:
  - name: store_id
    type: primary
    key: store_id
dimensions:
  - name: store_id
    type: number
    expr: store_id
measures:
  - name: store_count
    type: count
  - name: sales_per_store
    type: number
    expr: "{{stores.net_sales}} / NULLIF({{stores.store_count}}, 0)"
"#;

const SALES_VIEW: &str = r#"
name: sales
datasource: sim
table: sales
entities:
  - name: store_id
    type: foreign
    key: store_id
dimensions:
  - name: store_id
    type: number
    expr: store_id
  - name: date
    type: date
    expr: date
measures:
  - name: net_sales
    type: sum
    expr: net_sales
"#;

/// A probe over `stores`/`sales`, rooted on the composite's own-view input
/// and executing against real DuckDB — the engine's aliasing is the bug.
fn induced_probe(dataset: &std::path::Path) -> FitProbe {
    std::fs::write(dataset.join("stores.csv"), "store_id\n1\n2\n").unwrap();
    std::fs::write(
        dataset.join("sales.csv"),
        "store_id,date,net_sales\n1,2026-01-05,30.0\n2,2026-01-05,10.0\n",
    )
    .unwrap();
    let layer = oxy_airlayer_compat::SemanticLayer::new(
        vec![
            oxy_airlayer_compat::parse_view_yaml(STORES_VIEW).unwrap(),
            oxy_airlayer_compat::parse_view_yaml(SALES_VIEW).unwrap(),
        ],
        None,
    );
    let dialects = oxy_airlayer_compat::DatasourceDialectMap::from_config_databases(&[
        oxy_airlayer_compat::DatabaseConfig {
            name: super::DATASOURCE.to_string(),
            db_type: "duckdb".to_string(),
        },
    ]);
    let engine =
        oxy_airlayer_compat::SemanticEngine::from_semantic_layer(layer.clone(), dialects).unwrap();
    FitProbe {
        tree: oxy_airlayer_compat::engine::metric_tree::MetricTree::build(&layer),
        layer,
        engine: std::sync::Arc::new(engine),
        dataset_dir: dataset.to_string_lossy().into_owned(),
        roots: vec!["stores.store_count".to_string()],
        world: "induced".to_string(),
    }
}

/// `compile_query` rewrites `stores.net_sales` to its source and keys the row
/// `sales__net_sales`. Read without the layer, the level is looked up under
/// `stores__net_sales`, never found, and the composite reading it is left
/// without an input — so predict refuses it.
///
/// The outcome is deliberately not asserted: the engine rewrites an induced
/// name only when it is requested, not inside a composite's `expr`, so the
/// batch carrying `stores.sales_per_store`'s own column fails to compile. The
/// induced input is fetched in a batch of its own and survives that.
#[test]
fn a_composite_over_an_induced_measure_reads_its_input() {
    let dir = tempfile::TempDir::new().unwrap();
    let probe = induced_probe(dir.path());

    let (values, _) = probe.read_levels("sales.date", &probe.executor());
    oxy::connector::release_local_connection(dir.path().to_path_buf());

    assert_eq!(
        values.get("stores.net_sales"),
        Some(&40.0),
        "the induced input was never read back: {values:?}",
    );
}
