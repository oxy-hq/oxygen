//! `query`'s connector choice (a required, unknown or default database, and
//! a semantic query routed by its views' datasource) and semantic queries.

use std::collections::HashMap;
use std::sync::Arc;

use agentic_connector::DatabaseConnector;
use serde_json::json;

use super::query_tests::{Fake, account_catalog, run, tool};
use super::tools::{QueryDeps, QueryTool};
use crate::SemanticCatalog;

fn tool_with_default(conns: &[(&str, Arc<Fake>)], default: &str) -> QueryTool {
    let connectors: HashMap<String, Arc<dyn DatabaseConnector>> = conns
        .iter()
        .map(|(n, c)| (n.to_string(), c.clone() as Arc<dyn DatabaseConnector>))
        .collect();
    QueryTool::new(QueryDeps {
        connectors,
        catalog: None,
        default_connector: Some(default.to_owned()),
    })
}

/// Two views, each on its own datasource.
fn two_source_catalog() -> SemanticCatalog {
    let view = |name: &str, ds: &str| {
        crate::airlayer_compat::parse_view_yaml(&format!(
            "name: {name}\ntable: {name}\ndatasource: {ds}\n\
             dimensions:\n  - name: id\n    type: number\n    expr: id\n\
             measures:\n  - name: total\n    type: sum\n    expr: amount\n"
        ))
        .unwrap()
    };
    let layer = oxy_airlayer_compat::SemanticLayer::new(
        vec![view("sales", "wh_a"), view("costs", "wh_b")],
        None,
    );
    let dialects = oxy_airlayer_compat::DatasourceDialectMap::with_default(
        oxy_airlayer_compat::Dialect::DuckDB,
    );
    let engine = oxy_airlayer_compat::SemanticEngine::from_semantic_layer(layer, dialects).unwrap();
    SemanticCatalog::from_engine(engine)
}

#[tokio::test]
async fn query_uses_the_default_connector_when_several() {
    let (a, b) = (Fake::new(1), Fake::new(1));
    let t = tool_with_default(&[("a", a.clone()), ("b", b.clone())], "b");
    run(&t, json!({"sql": "select 1"})).await.unwrap();
    assert!(a.seen().is_empty());
    assert_eq!(b.seen(), vec!["select 1"]);
    // Without a default, today's refusal stands.
    let t = tool(&[("a", a), ("b", b)], None);
    let err = run(&t, json!({"sql": "select 1"})).await.unwrap_err();
    assert_eq!(err.error, "database is required");
}

#[tokio::test]
async fn semantic_query_routes_by_view_datasource() {
    let (a, b) = (Fake::new(1), Fake::new(1));
    let t = tool(
        &[("wh_a", a.clone()), ("wh_b", b.clone())],
        Some(two_source_catalog()),
    );
    run(&t, json!({"semantic": {"measures": ["costs.total"]}}))
        .await
        .unwrap();
    assert!(a.seen().is_empty());
    assert_eq!(b.seen().len(), 1);
    // A `database` that contradicts the routing is refused, not obeyed.
    let err = run(
        &t,
        json!({"semantic": {"measures": ["costs.total"]}, "database": "wh_a"}),
    )
    .await
    .unwrap_err();
    assert!(err.error.contains("wh_b"), "{}", err.error);
}

#[tokio::test]
async fn semantic_query_over_two_datasources_fails() {
    let (a, b) = (Fake::new(1), Fake::new(1));
    let t = tool(
        &[("wh_a", a.clone()), ("wh_b", b.clone())],
        Some(two_source_catalog()),
    );
    let q = json!({"semantic": {"measures": ["sales.total", "costs.total"]}});
    let err = run(&t, q).await.unwrap_err();
    assert!(
        err.error.contains("wh_a") && err.error.contains("wh_b"),
        "{}",
        err.error
    );
    assert!(a.seen().is_empty() && b.seen().is_empty());
}

#[tokio::test]
async fn query_needs_a_database_when_there_are_several() {
    let t = tool(
        &[("warehouse", Fake::new(1)), ("sales", Fake::new(1))],
        None,
    );
    let err = run(&t, json!({"sql": "select 1"})).await.unwrap_err();
    assert_eq!(err.error, "database is required");
    assert_eq!(err.candidates, vec!["sales", "warehouse"]);
}

#[tokio::test]
async fn query_names_an_unknown_database_with_candidates() {
    let sales = Fake::new(1);
    let t = tool(
        &[("warehouse", Fake::new(1)), ("sales", sales.clone())],
        None,
    );
    let err = run(&t, json!({"sql": "select 1", "database": "warehose"}))
        .await
        .unwrap_err();
    assert_eq!(
        err.candidates.first().map(String::as_str),
        Some("warehouse")
    );
    let ok = run(&t, json!({"sql": "select 1", "database": "sales"})).await;
    assert!(ok.is_ok() && sales.seen() == vec!["select 1"]);
}

#[tokio::test]
async fn semantic_query_reports_unknown_members_with_candidates() {
    let db = Fake::new(3);
    let t = tool(&[("warehouse", db.clone())], Some(account_catalog()));
    let err = run(
        &t,
        json!({"semantic": {"measures": ["account.totl_amount"]}}),
    )
    .await
    .unwrap_err();
    assert!(err.error.contains("account.totl_amount"), "{}", err.error);
    assert!(
        err.candidates.iter().any(|c| c == "account.total_amount"),
        "{:?}",
        err.candidates
    );
    assert!(db.seen().is_empty());
}

#[tokio::test]
async fn semantic_query_runs_the_compiled_sql() {
    let db = Fake::new(3);
    let t = tool(&[("warehouse", db.clone())], Some(account_catalog()));
    let reply = run(
        &t,
        json!({"semantic": {"measures": ["account.total_amount"], "dimensions": ["account.district_id"]}}),
    )
    .await
    .unwrap();
    let seen = db.seen();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].to_lowercase().contains("sum("), "{}", seen[0]);
    assert!(reply.text.contains(&seen[0]), "{}", reply.text);
    assert_eq!(reply.value["sql"], seen[0].as_str());
    assert_eq!(reply.artifacts[0].payload["source"], "semantic");
}

#[tokio::test]
async fn query_offers_no_candidates_for_a_name_nothing_resembles() {
    let t = tool(
        &[("warehouse", Fake::new(1)), ("sales", Fake::new(1))],
        None,
    );
    let err = run(&t, json!({"sql": "select 1", "database": "qqqqqq"}))
        .await
        .unwrap_err();
    assert!(err.candidates.is_empty(), "{:?}", err.candidates);
}
