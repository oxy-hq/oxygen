//! The metric-tree tools on react: offered exactly when a runner is given,
//! described for react, and run through the FSM's executor over the runner.

use std::collections::HashMap;
use std::sync::Arc;

use agentic_core::react::Tool;
use oxy_airlayer_compat::engine::metric_tree_ops::{
    BenchmarkStatistic, ExplainConfig, ExplainResult, OpportunityResult,
};
use oxy_airlayer_compat::engine::query::QueryFilter;
use oxy_airlayer_compat::{DatabaseConfig, SemanticLayer};
use serde_json::json;

use super::files::WorkspaceFiles;
use super::query_tests::cx;
use super::run::{ReactAnalytics, ReactInputs};
use super::tools::MAX_TREE_REPLY_CHARS;
use crate::metric_tree_runner::{MetricTreeRunner, MetricTreeRunnerError};
use crate::tools::METRIC_TREE_TOOL_NAMES;

/// A runner that only hands back a layer: `predict_impact` and
/// `metric_sensitivity` are pure ops over it, so nothing here queries.
struct LayerOnly(SemanticLayer);

fn unused() -> MetricTreeRunnerError {
    MetricTreeRunnerError::Op("not called in these tests".into())
}

#[async_trait::async_trait]
impl MetricTreeRunner for LayerOnly {
    async fn load_layer(&self) -> Result<SemanticLayer, MetricTreeRunnerError> {
        Ok(self.0.clone())
    }
    async fn list_databases(&self) -> Vec<DatabaseConfig> {
        vec![]
    }
    async fn run_explain(
        &self,
        _: String,
        _: String,
        _: (String, String),
        _: (String, String),
        _: Vec<QueryFilter>,
        _: ExplainConfig,
    ) -> Result<ExplainResult, MetricTreeRunnerError> {
        // bca: suppress(nargs) -- the MetricTreeRunner trait fixes this signature
        Err(unused())
    }
    async fn run_opportunity(
        &self,
        _: String,
        _: String,
        _: (String, String),
        _: BenchmarkStatistic,
    ) -> Result<OpportunityResult, MetricTreeRunnerError> {
        Err(unused())
    }
    async fn get_dimension_values(
        &self,
        _: String,
        _: String,
        _: u32,
    ) -> Result<Vec<String>, MetricTreeRunnerError> {
        Err(unused())
    }
    async fn run_time_series(
        &self,
        _: String,
        _: String,
        _: String,
        _: (String, String),
        _: Vec<QueryFilter>,
        _: Option<String>,
    ) -> Result<Vec<(String, f64)>, MetricTreeRunnerError> {
        // bca: suppress(nargs) -- the MetricTreeRunner trait fixes this signature
        Err(unused())
    }
}

/// `orders.profit = orders.revenue - orders.cost`: a tree with one composite.
fn profit_runner() -> Arc<dyn MetricTreeRunner> {
    let view = oxy_airlayer_compat::parse_view_yaml(
        r#"
name: orders
table: public.orders
dialect: postgres
measures:
  - name: revenue
    type: sum
    expr: amount
  - name: cost
    type: sum
    expr: cost
  - name: profit
    type: number
    expr: "{{orders.revenue}} - {{orders.cost}}"
"#,
    )
    .expect("view parses");
    Arc::new(LayerOnly(SemanticLayer::new(vec![view], None)))
}

fn analytics() -> ReactAnalytics {
    ReactAnalytics::new(ReactInputs {
        instructions: String::new(),
        databases: HashMap::new(),
        catalog: None,
        default_connector: None,
        files: Arc::new(WorkspaceFiles::from_entries(vec![])),
        semantic_only: false,
        advisor: false,
        preagg: None,
    })
}

fn names(run: &ReactAnalytics) -> Vec<&'static str> {
    run.toolset.tools.iter().map(|t| t.def().name).collect()
}

fn tool(run: &ReactAnalytics, name: &str) -> Arc<dyn Tool> {
    let found = run.toolset.tools.iter().find(|t| t.def().name == name);
    found.expect("the tool is offered").clone()
}

/// No runner, no tools and no rule; a runner adds the FSM's four names and
/// the rule, and changes nothing else.
#[test]
fn the_tree_tools_are_offered_exactly_when_a_runner_is_given() {
    let without = analytics();
    for name in METRIC_TREE_TOOL_NAMES {
        assert!(
            !names(&without).contains(name),
            "{name} offered with no runner"
        );
    }
    assert!(!without.toolset.system_prompt.contains("explain_metric"));

    let with = analytics().with_metric_tree(profit_runner());
    let all = names(&with);
    assert_eq!(&all[..names(&without).len()], &names(&without)[..]);
    assert_eq!(&all[names(&without).len()..], METRIC_TREE_TOOL_NAMES);
    let prompt = &with.toolset.system_prompt;
    assert!(
        prompt.starts_with(&without.toolset.system_prompt),
        "{prompt}"
    );
    assert!(
        prompt.contains("call `explain_metric` with both periods"),
        "{prompt}"
    );
    assert!(
        prompt.contains("They have no result id to cite."),
        "{prompt}"
    );
}

/// React has no `search_catalog`, and `deep` is optional for the executor.
#[test]
fn the_tree_tools_are_described_for_react() {
    let with = analytics().with_metric_tree(profit_runner());
    for name in METRIC_TREE_TOOL_NAMES {
        let def = tool(&with, name).def();
        let text = format!("{} {}", def.description, def.parameters);
        assert!(!text.contains("search_catalog"), "{name}: {text}");
        assert!(!def.strict, "{name}");
    }
    let explain = tool(&with, "explain_metric").def();
    let required = explain.parameters["required"].as_array().unwrap();
    assert!(!required.contains(&json!("deep")), "{required:?}");
    assert!(
        required.contains(&json!("previous_period_end")),
        "{required:?}"
    );
    assert!(
        explain
            .description
            .starts_with("Explain why a measure changed")
    );
}

/// The call runs the FSM's op over the runner's layer: a change to revenue
/// reaches profit.
#[tokio::test]
async fn predict_impact_runs_through_the_runner() {
    let with = analytics().with_metric_tree(profit_runner());
    let args = json!({"changes": [{"measure": "orders.revenue", "delta": 100.0}]});
    let reply = tool(&with, "predict_impact")
        .call(args, &cx())
        .await
        .expect("revenue is in the tree");
    assert!(reply.text.contains("orders.profit"), "{}", reply.text);
    assert_eq!(reply.value, json!({"truncated": false}));
}

/// A failed op names the tool and says what to try next.
#[tokio::test]
async fn a_failed_tree_call_says_what_to_try_next() {
    let with = analytics().with_metric_tree(profit_runner());
    let sensitivity = tool(&with, "metric_sensitivity");
    let err = sensitivity
        .call(json!({"target": "orders.margin"}), &cx())
        .await
        .expect_err("no such measure");
    assert!(
        err.error.starts_with("metric_sensitivity failed:"),
        "{}",
        err.error
    );
    assert!(err.error.contains("orders.margin"), "{}", err.error);
    assert!(err.hint.unwrap().contains("`search`"));

    let err = sensitivity
        .call(json!({}), &cx())
        .await
        .expect_err("no target");
    assert!(err.error.contains("missing 'target'"), "{}", err.error);
    assert!(err.hint.unwrap().contains("`metric_sensitivity` schema"));
}

/// A lever conflict is not a shape error: the hint says to drop one of the
/// two changes, not to check the schema.
#[tokio::test]
async fn a_lever_conflict_hints_to_change_one_measure() {
    let with = analytics().with_metric_tree(profit_runner());
    let args = json!({"changes": [
        {"measure": "orders.revenue", "delta": 100.0},
        {"measure": "orders.profit", "delta": 50.0},
    ]});
    let err = tool(&with, "predict_impact")
        .call(args, &cx())
        .await
        .expect_err("revenue is upstream of profit");
    assert!(err.error.contains("ambiguous scenario"), "{}", err.error);
    let hint = err.hint.unwrap();
    assert!(hint.contains("not both"), "{hint}");
    assert!(!hint.contains("schema"), "{hint}");
}

/// A long result is cut, and the cut is stated.
#[test]
fn a_long_tree_result_is_cut_and_says_so() {
    let long = json!({"nodes": "x".repeat(MAX_TREE_REPLY_CHARS * 2)});
    let reply = super::tools::tree_reply(long);
    assert!(
        reply.text.contains("… result cut at 20000 of"),
        "{}",
        &reply.text[..80]
    );
    assert_eq!(reply.value, json!({"truncated": true}));
}
