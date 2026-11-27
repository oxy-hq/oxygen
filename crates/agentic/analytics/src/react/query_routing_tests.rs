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
        semantic_only: false,
        verified: Default::default(),
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

fn semantic_only_tool(db: Arc<Fake>) -> QueryTool {
    let connectors: HashMap<String, Arc<dyn DatabaseConnector>> =
        HashMap::from([("warehouse".to_string(), db as Arc<dyn DatabaseConnector>)]);
    QueryTool::new(QueryDeps {
        connectors,
        catalog: Some(Arc::new(account_catalog())),
        default_connector: None,
        semantic_only: true,
        verified: Default::default(),
    })
}

/// A `semantic_only` agent's `query` refuses raw `sql` before any database
/// is reached, with or without `semantic` beside it, and still runs a
/// semantic query.
#[tokio::test]
async fn semantic_only_refuses_sql_and_runs_semantic() {
    let db = Fake::new(3);
    let t = semantic_only_tool(db.clone());
    for args in [
        json!({"sql": "select 1"}),
        json!({"sql": "select 1", "semantic": {"measures": ["account.total_amount"]}}),
    ] {
        let err = run(&t, args).await.unwrap_err();
        assert!(err.error.contains("raw `sql` is refused"), "{}", err.error);
        assert!(err.hint.as_deref().unwrap_or("").contains("`semantic`"));
    }
    assert!(
        db.seen().is_empty(),
        "a refused sql never reaches the database"
    );
    let reply = run(
        &t,
        json!({"semantic": {"measures": ["account.total_amount"]}}),
    )
    .await
    .unwrap();
    assert_eq!(reply.artifacts[0].payload["source"], "semantic");
}

/// The schema a `semantic_only` agent's model sees has no `sql` parameter;
/// the default tool keeps it.
#[test]
fn semantic_only_def_drops_the_sql_parameter() {
    use agentic_core::react::Tool;
    let def = semantic_only_tool(Fake::new(1)).def();
    let props = &def.parameters["properties"];
    assert!(props.get("sql").is_none(), "{props}");
    assert!(props.get("semantic").is_some());
    assert!(!def.description.contains("`sql`"), "{}", def.description);
    let plain = tool(&[("warehouse", Fake::new(1))], None).def();
    assert!(plain.parameters["properties"].get("sql").is_some());
}

/// Every op the schema offers deserializes with the field its branch
/// requires, and the schema offers every op the deserializer accepts: the
/// schema and `SemanticQueryConfig` cannot drift apart.
#[test]
fn semantic_schema_ops_match_what_deserializes() {
    use agentic_core::react::Tool;
    use agentic_semantic::config::SemanticQueryConfig;
    let def = tool(&[("warehouse", Fake::new(1))], Some(account_catalog())).def();
    let semantic = &def.parameters["properties"]["semantic"];
    let branches = semantic["properties"]["filters"]["items"]["anyOf"]
        .as_array()
        .expect("filter branches");
    let mut offered: Vec<String> = Vec::new();
    for b in branches {
        let required: Vec<&str> = b["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        for op in b["properties"]["op"]["enum"].as_array().unwrap() {
            let op = op.as_str().unwrap();
            let mut filter = json!({"field": "account.district_id", "op": op});
            for f in &required {
                filter[*f] = match *f {
                    "values" => json!([1, 2]),
                    "field" | "op" => continue,
                    _ => json!("2020-01-01"),
                };
            }
            let config = json!({"measures": ["account.total_amount"], "filters": [filter]});
            serde_json::from_value::<SemanticQueryConfig>(config.clone())
                .unwrap_or_else(|e| panic!("{op}: {e} for {config}"));
            offered.push(op.to_owned());
        }
    }
    // An op the schema does not offer is refused, so the enum is complete
    // when its count matches the deserializer's variants (11).
    offered.sort();
    offered.dedup();
    assert_eq!(offered.len(), 11, "{offered:?}");
    for g in semantic["properties"]["time_dimensions"]["items"]["properties"]["granularity"]["enum"]
        .as_array()
        .unwrap()
    {
        let config = json!({"time_dimensions": [{"dimension": "account.date", "granularity": g}]});
        serde_json::from_value::<SemanticQueryConfig>(config)
            .unwrap_or_else(|e| panic!("{g}: {e}"));
    }
}

#[tokio::test]
async fn invalid_semantic_query_names_each_op_and_its_field() {
    let db = Fake::new(1);
    let t = tool(&[("warehouse", db.clone())], Some(account_catalog()));
    let err = run(
        &t,
        json!({"semantic": {"measures": ["account.total_amount"],
            "filters": [{"field": "account.district_id", "op": "equals", "value": 1}]}}),
    )
    .await
    .unwrap_err();
    assert!(err.error.contains("equals"), "{}", err.error);
    let hint = err.hint.unwrap_or_default();
    assert!(hint.contains("eq") && hint.contains("`values`"), "{hint}");
    assert!(db.seen().is_empty());
}

#[tokio::test]
async fn neither_sql_nor_semantic_names_what_was_sent() {
    let t = tool(&[("warehouse", Fake::new(1))], Some(account_catalog()));
    let err = run(&t, json!({})).await.unwrap_err();
    assert!(err.error.ends_with("got no arguments"), "{}", err.error);
    let err = run(
        &t,
        json!({"measures": ["account.total_amount"], "semantic": null}),
    )
    .await
    .unwrap_err();
    assert!(err.error.contains("measures (an array)"), "{}", err.error);
    assert!(err.error.contains("semantic (null)"), "{}", err.error);
}

/// A model often sends a semantic query's fields at the top level instead of
/// under `semantic` (seen on the first call of most BIRD questions). With no
/// `sql` and no `semantic` key at all, `query` reads the arguments
/// themselves as the semantic query.
#[tokio::test]
async fn semantic_fields_at_the_top_level_run_as_a_semantic_query() {
    let db = Fake::new(3);
    let t = semantic_only_tool(db.clone());
    let reply = run(
        &t,
        json!({"measures": ["account.total_amount"], "dimensions": ["account.district_id"], "limit": 5}),
    )
    .await
    .unwrap();
    assert_eq!(reply.artifacts[0].payload["source"], "semantic");
    assert_eq!(db.seen().len(), 1);
    let err = run(&t, json!({"limit": 5})).await.unwrap_err();
    assert!(err.error.contains("got keys: limit"), "{}", err.error);
}
