//! The `chart` tool's check: what it draws is the rows as stored, and what it
//! refuses says how to fix the query or the spec.

use std::sync::Arc;

use agentic_core::react::Tool;
use agentic_runtime::react::{ResultStore, StoredResult, column_kinds};
use serde_json::{Value, json};

use super::query_tests::cx;
use super::tools::{ChartKind, ChartSpec, ChartTool, MAX_CHART_ROWS, check_chart};
use crate::AnalyticsEvent;

fn stored(columns: &[&str], rows: Vec<Vec<Value>>) -> StoredResult {
    StoredResult {
        sql: "select 1".into(),
        columns: columns.iter().map(|c| c.to_string()).collect(),
        column_kinds: column_kinds(columns.len(), &rows),
        row_count: rows.len(),
        rows: Arc::new(rows),
        truncated: false,
    }
}

fn spec(kind: ChartKind, x: &str, y: &[&str], series: Option<&str>) -> ChartSpec {
    ChartSpec {
        result_id: "r1".into(),
        kind,
        x: x.into(),
        y: y.iter().map(|c| c.to_string()).collect(),
        series: series.map(str::to_owned),
        title: None,
    }
}

/// The bug this replaces: a hidden `SUM … GROUP BY` turned signs and
/// decimals into other numbers. Points are the stored values, unchanged.
#[test]
fn negatives_and_decimals_are_drawn_as_stored() {
    let rows = vec![
        vec![json!("2026-01"), json!(-12.5), json!(0.1)],
        vec![json!("2026-02"), json!(0.30000000000000004), json!(-0.0001)],
        vec![
            json!("2026-03"),
            json!(null),
            json!(-9_007_199_254_740_991_i64),
        ],
    ];
    let r = stored(&["month", "delta", "rate"], rows.clone());
    let points = check_chart(
        &spec(ChartKind::Line, "month", &["delta", "rate"], None),
        &r,
    )
    .unwrap();
    assert_eq!(points, rows, "a null stays null, never 0");
}

#[test]
fn a_repeated_x_is_refused_with_an_aggregate_hint() {
    let r = stored(
        &["region", "revenue"],
        vec![
            vec![json!("east"), json!(5)],
            vec![json!("west"), json!(-3)],
            vec![json!("east"), json!(7)],
        ],
    );
    let err = check_chart(&spec(ChartKind::Bar, "region", &["revenue"], None), &r).unwrap_err();
    assert!(err.error.contains("rows 1 and 3"), "{}", err.error);
    assert!(err.hint.unwrap().contains("GROUP BY"));
}

/// With a series, x repeats once per series value; only a repeated pair is
/// refused.
#[test]
fn a_series_makes_x_repeat_but_never_the_pair() {
    let rows = vec![
        vec![json!("jan"), json!("east"), json!(1)],
        vec![json!("jan"), json!("west"), json!(-2)],
        vec![json!("feb"), json!("east"), json!(3)],
    ];
    let mut r = stored(&["month", "region", "n"], rows.clone());
    let s = spec(ChartKind::Line, "month", &["n"], Some("region"));
    let points = check_chart(&s, &r).unwrap();
    assert_eq!(points[1], vec![json!("jan"), json!("west"), json!(-2)]);
    let mut dup = rows;
    dup.push(vec![json!("jan"), json!("west"), json!(4)]);
    r = stored(&["month", "region", "n"], dup);
    let err = check_chart(&s, &r).unwrap_err();
    assert!(err.error.contains("series"), "{}", err.error);
    let two_y = spec(ChartKind::Line, "month", &["n", "n"], Some("region"));
    assert!(check_chart(&two_y, &r).is_err(), "a series takes one y");
}

#[test]
fn a_scatter_allows_repeated_x_but_needs_a_numeric_one() {
    let r = stored(
        &["price", "units", "sku"],
        vec![
            vec![json!(2.5), json!(10), json!("a")],
            vec![json!(2.5), json!(-4), json!("b")],
        ],
    );
    assert_eq!(
        check_chart(&spec(ChartKind::Scatter, "price", &["units"], None), &r)
            .unwrap()
            .len(),
        2
    );
    let err = check_chart(&spec(ChartKind::Scatter, "sku", &["units"], None), &r).unwrap_err();
    assert!(err.error.contains("not numeric"), "{}", err.error);
}

#[test]
fn too_many_rows_or_a_truncated_result_is_refused() {
    let rows: Vec<Vec<Value>> = (0..=MAX_CHART_ROWS)
        .map(|i| vec![json!(i), json!(i)])
        .collect();
    let r = stored(&["i", "n"], rows);
    let err = check_chart(&spec(ChartKind::Line, "i", &["n"], None), &r).unwrap_err();
    assert!(err.error.contains("501 rows"), "{}", err.error);
    let mut small = stored(&["i", "n"], vec![vec![json!(1), json!(1)]]);
    small.truncated = true;
    assert!(check_chart(&spec(ChartKind::Line, "i", &["n"], None), &small).is_err());
    let empty = stored(&["i", "n"], vec![]);
    assert!(check_chart(&spec(ChartKind::Line, "i", &["n"], None), &empty).is_err());
}

#[test]
fn a_column_is_named_once_and_y_is_numeric() {
    let r = stored(
        &["id", "id", "name"],
        vec![vec![json!(1), json!(2), json!("a")]],
    );
    let missing = check_chart(&spec(ChartKind::Bar, "nope", &["id"], None), &r).unwrap_err();
    assert_eq!(missing.candidates, ["id", "id", "name"]);
    let shared = check_chart(&spec(ChartKind::Bar, "name", &["id"], None), &r).unwrap_err();
    assert!(shared.hint.unwrap().contains("AS"));
    let r = stored(&["a", "b"], vec![vec![json!("x"), json!("text")]]);
    let text_y = check_chart(&spec(ChartKind::Bar, "a", &["b"], None), &r).unwrap_err();
    assert!(text_y.error.contains("not numeric"), "{}", text_y.error);
    assert!(check_chart(&spec(ChartKind::Bar, "a", &[], None), &r).is_err());
}

/// A pie has one measure and no sign to show.
#[test]
fn a_pie_refuses_negatives_series_and_two_measures() {
    let r = stored(
        &["k", "v", "w"],
        vec![
            vec![json!("a"), json!(3), json!(1)],
            vec![json!("b"), json!(-1), json!(1)],
        ],
    );
    let neg = check_chart(&spec(ChartKind::Pie, "k", &["v"], None), &r).unwrap_err();
    assert!(neg.error.contains("negative"), "{}", neg.error);
    assert!(check_chart(&spec(ChartKind::Pie, "k", &["w"], None), &r).is_ok());
    assert!(check_chart(&spec(ChartKind::Pie, "k", &["w", "v"], None), &r).is_err());
    assert!(check_chart(&spec(ChartKind::Pie, "k", &["w"], Some("k")), &r).is_err());
}

/// A pie with a repeated label would draw two slices for one category, each
/// a part of its total: refused like any other kind but a scatter.
#[test]
fn a_pie_refuses_a_repeated_label() {
    let r = stored(
        &["category", "revenue"],
        vec![
            vec![json!("A"), json!(3)],
            vec![json!("B"), json!(2)],
            vec![json!("A"), json!(4)],
        ],
    );
    let err = check_chart(&spec(ChartKind::Pie, "category", &["revenue"], None), &r).unwrap_err();
    assert!(err.error.contains("rows 1 and 3"), "{}", err.error);
    assert!(err.hint.unwrap().contains("GROUP BY"));
}

/// A checked spec becomes one `result_chart` row the FSM's event type reads
/// back; a bad spec or an unknown result is a failure the model can fix.
#[tokio::test]
async fn the_tool_emits_a_result_chart_row() {
    let store = Arc::new(ResultStore::new());
    let id = store.put(
        "select m, n from t".into(),
        vec!["m".into(), "n".into()],
        vec![
            vec![json!("jan"), json!(-1.5)],
            vec![json!("feb"), json!(2)],
        ],
    );
    let tool = ChartTool::new(store);
    let args = json!({"result_id": id, "kind": "bar", "x": "m", "y": ["n"], "title": "N"});
    let reply = tool.call(args, &cx()).await.unwrap();
    assert!(
        reply.text.contains("bar of n by m from r1, 2 rows"),
        "{}",
        reply.text
    );
    let [row] = reply.artifacts.as_slice() else {
        panic!("one artifact")
    };
    assert_eq!(row.event_type, "result_chart");
    let mut tagged = row.payload.clone();
    tagged["event_type"] = json!("result_chart");
    let AnalyticsEvent::ResultChart {
        rows, kind, title, ..
    } = serde_json::from_value::<AnalyticsEvent>(tagged).unwrap()
    else {
        panic!("a result_chart event")
    };
    assert_eq!((kind, title.as_deref()), (ChartKind::Bar, Some("N")));
    assert_eq!(
        rows,
        vec![
            vec![json!("jan"), json!(-1.5)],
            vec![json!("feb"), json!(2)]
        ]
    );

    let unknown = json!({"result_id": "r9", "kind": "bar", "x": "m", "y": ["n"]});
    assert!(tool.call(unknown, &cx()).await.is_err());
    let extra = json!({"result_id": "r1", "kind": "bar", "x": "m", "y": ["n"], "stack": true});
    let err = tool.call(extra, &cx()).await.unwrap_err();
    assert!(err.error.contains("stack"), "{}", err.error);
}
