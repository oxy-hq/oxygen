//! The anomaly tools on react: offered exactly when a store and a runner are
//! given, described for react, and run through the FSM's executor over both,
//! scoped to the host's workspace.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use agentic_core::react::{Effect, Tool};
use oxy_airlayer_compat::engine::metric_tree_ops::{
    BenchmarkStatistic, ExplainConfig, ExplainResult, OpportunityResult,
};
use oxy_airlayer_compat::engine::query::QueryFilter;
use oxy_airlayer_compat::{DatabaseConfig, SemanticLayer};
use serde_json::{Value, json};
use uuid::Uuid;

use super::files::WorkspaceFiles;
use super::query_tests::cx;
use super::run::{ReactAnalytics, ReactInputs};
use super::tools::AnomalyPorts;
use crate::anomaly_store::{
    AnomalyFilter, AnomalyRecord, AnomalyStore, AnomalyStoreError, DetectAndUpsertResult,
    DetectRequest,
};
use crate::metric_tree_runner::{MetricTreeRunner, MetricTreeRunnerError};

const ANOMALY_TOOLS: [&str; 3] = ["list_anomalies", "detect_anomalies", "explain_anomaly"];

const WORKSPACE: Uuid = Uuid::from_u128(7);
const STORED: Uuid = Uuid::from_u128(42);

fn unused() -> MetricTreeRunnerError {
    MetricTreeRunnerError::Op("not called in these tests".into())
}

/// A runner whose only data is a two-day series.
struct TwoDays;

#[async_trait::async_trait]
impl MetricTreeRunner for TwoDays {
    async fn load_layer(&self) -> Result<SemanticLayer, MetricTreeRunnerError> {
        Err(unused())
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
        Ok(vec![
            ("2026-06-29T00:00:00Z".into(), 10.0),
            ("2026-06-30T00:00:00Z".into(), 90.0),
        ])
    }
}

/// A period bound the store refuses.
const UNREADABLE: &str = "June 2026";

/// A store holding one anomaly (`STORED`), recording the workspace and the
/// filter of every list, and the observations of every detect.
#[derive(Default)]
struct OneAnomaly {
    lists: Mutex<Vec<(Uuid, AnomalyFilter)>>,
    detected: Mutex<Vec<(Uuid, usize)>>,
}

fn stored() -> AnomalyRecord {
    AnomalyRecord {
        id: STORED,
        measure: "orders.total_order_value".into(),
        time_dimension: "orders.order_date".into(),
        granularity: "day".into(),
        period_start: "2026-06-12T00:00:00+00:00".into(),
        period_end: "2026-06-13T00:00:00+00:00".into(),
        observed: 900.0,
        expected: 400.0,
        lower: 300.0,
        upper: 500.0,
        z_score: 4.2,
        severity: "high".into(),
        status: "new".into(),
        seasonal_period: Some(7),
        filters: None,
    }
}

#[async_trait::async_trait]
impl AnomalyStore for OneAnomaly {
    async fn list(
        &self,
        workspace_id: Uuid,
        filter: AnomalyFilter,
    ) -> Result<Vec<AnomalyRecord>, AnomalyStoreError> {
        // As `OxyAnomalyStore` does for a bound in neither format.
        if filter.period_start_gte.as_deref() == Some(UNREADABLE) {
            return Err(AnomalyStoreError::InvalidFilter(format!(
                "period_start {UNREADABLE:?} is not a date (YYYY-MM-DD) or an RFC 3339 timestamp"
            )));
        }
        self.lists.lock().unwrap().push((workspace_id, filter));
        Ok(vec![stored()])
    }
    async fn get(&self, id: Uuid, _: Uuid) -> Result<Option<AnomalyRecord>, AnomalyStoreError> {
        Ok((id == STORED).then(stored))
    }
    async fn detect_and_upsert(
        &self,
        workspace_id: Uuid,
        request: DetectRequest,
    ) -> Result<DetectAndUpsertResult, AnomalyStoreError> {
        let total = request.observations.len();
        self.detected.lock().unwrap().push((workspace_id, total));
        Ok(DetectAndUpsertResult {
            anomalies: vec![],
            total_observations: total,
            message: Some(format!(
                "Not enough data: {total} observations, need at least 56"
            )),
            missing_periods: vec![],
        })
    }
    async fn get_explain_cache(
        &self,
        _: Uuid,
        _: Uuid,
    ) -> Result<Option<Value>, AnomalyStoreError> {
        Ok(None)
    }
    async fn set_explain_cache(&self, _: Uuid, _: Uuid, _: Value) -> Result<(), AnomalyStoreError> {
        Ok(())
    }
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

fn with_anomalies(store: Arc<OneAnomaly>) -> ReactAnalytics {
    analytics().with_anomalies(AnomalyPorts {
        store,
        runner: Arc::new(TwoDays),
        workspace_id: WORKSPACE,
    })
}

fn names(run: &ReactAnalytics) -> Vec<&'static str> {
    run.toolset.tools.iter().map(|t| t.def().name).collect()
}

fn tool(run: &ReactAnalytics, name: &str) -> Arc<dyn Tool> {
    let found = run.toolset.tools.iter().find(|t| t.def().name == name);
    found.expect("the tool is offered").clone()
}

fn june(measure: &str) -> Value {
    json!({
        "measure": measure,
        "time_dimension": "orders.order_date",
        "granularity": "day",
        "period_start": "2026-06-01",
        "period_end": "2026-06-30",
    })
}

/// Without the ports, no tools and no rule; with them, the FSM's three names
/// and the rule, and nothing else changes.
#[test]
fn the_anomaly_tools_are_offered_exactly_when_the_ports_are_given() {
    let without = analytics();
    for name in ANOMALY_TOOLS {
        assert!(!names(&without).contains(&name), "{name} offered");
    }
    assert!(!without.toolset.system_prompt.contains("list_anomalies"));

    let with = with_anomalies(Arc::default());
    let all = names(&with);
    assert_eq!(&all[..names(&without).len()], &names(&without)[..]);
    assert_eq!(&all[names(&without).len()..], ANOMALY_TOOLS);
    let prompt = &with.toolset.system_prompt;
    assert!(
        prompt.starts_with(&without.toolset.system_prompt),
        "{prompt}"
    );
    assert!(prompt.contains("call `list_anomalies` first"), "{prompt}");
}

/// React has no `search_catalog`; only `list_anomalies` is a pure read.
#[test]
fn the_anomaly_tools_are_described_for_react() {
    let with = with_anomalies(Arc::default());
    for name in ANOMALY_TOOLS {
        let t = tool(&with, name);
        let def = t.def();
        let text = format!("{} {}", def.description, def.parameters);
        assert!(!text.contains("search_catalog"), "{name}: {text}");
        assert!(!def.strict, "{name}");
        let effect = if name == "list_anomalies" {
            Effect::Read
        } else {
            Effect::Write
        };
        assert_eq!(t.effect(), effect, "{name}");
    }
    let detect = tool(&with, "detect_anomalies").def();
    assert!(
        detect.description.contains("scores every bucket")
            && detect.description.contains("`missing_periods`"),
        "{}",
        detect.description
    );
}

/// `list_anomalies` reads the store in the host's workspace, with the
/// question's measure and period, and the stored id comes back.
#[tokio::test]
async fn list_anomalies_reads_the_store_in_the_hosts_workspace() {
    let store = Arc::new(OneAnomaly::default());
    let with = with_anomalies(store.clone());
    let reply = tool(&with, "list_anomalies")
        .call(june("orders.total_order_value"), &cx())
        .await
        .expect("the store answers");
    assert!(reply.text.contains(&STORED.to_string()), "{}", reply.text);
    assert!(reply.text.contains("\"count\":1"), "{}", reply.text);
    let lists = store.lists.lock().unwrap();
    let (workspace, filter) = &lists[0];
    assert_eq!(*workspace, WORKSPACE);
    assert_eq!(filter.measure.as_deref(), Some("orders.total_order_value"));
    assert_eq!(filter.period_start_gte.as_deref(), Some("2026-06-01"));
}

/// A bound the store cannot read comes back as bad params naming the
/// accepted formats, with the hint to fix the argument, not to try `query`.
#[tokio::test]
async fn an_unreadable_period_bound_is_bad_params_for_the_model() {
    let store = Arc::new(OneAnomaly::default());
    let with = with_anomalies(store.clone());
    let mut args = june("orders.total_order_value");
    args["period_start"] = json!(UNREADABLE);
    let err = tool(&with, "list_anomalies")
        .call(args, &cx())
        .await
        .expect_err("the store refuses the bound");
    assert!(err.error.contains("bad params"), "{}", err.error);
    assert!(err.error.contains("YYYY-MM-DD"), "{}", err.error);
    assert!(err.hint.unwrap().contains("fix what the error names"));
    assert!(store.lists.lock().unwrap().is_empty());
}

/// `detect_anomalies` hands the runner's series to the store, and the
/// store's "not enough data" message reaches the model.
#[tokio::test]
async fn detect_anomalies_scores_the_runners_series_in_the_store() {
    let store = Arc::new(OneAnomaly::default());
    let with = with_anomalies(store.clone());
    let reply = tool(&with, "detect_anomalies")
        .call(june("orders.total_order_value"), &cx())
        .await
        .expect("the store answers");
    assert!(reply.text.contains("Not enough data"), "{}", reply.text);
    assert_eq!(*store.detected.lock().unwrap(), vec![(WORKSPACE, 2)]);
}

/// A bad id says what to pass; an unknown id says where to find a current one.
#[tokio::test]
async fn a_failed_anomaly_call_says_what_to_try_next() {
    let with = with_anomalies(Arc::default());
    let explain = tool(&with, "explain_anomaly");
    let err = explain
        .call(json!({"anomaly_id": "june"}), &cx())
        .await
        .expect_err("not a uuid");
    assert!(
        err.error.starts_with("explain_anomaly failed:"),
        "{}",
        err.error
    );
    assert!(err.hint.unwrap().contains("`anomaly_id`"));

    let missing = Uuid::from_u128(9).to_string();
    let err = explain
        .call(json!({"anomaly_id": missing}), &cx())
        .await
        .expect_err("no such anomaly");
    assert!(err.error.contains("not found"), "{}", err.error);
    assert!(err.hint.unwrap().contains("`list_anomalies`"));

    let err = tool(&with, "list_anomalies")
        .call(json!({"measure": "orders.total_order_value"}), &cx())
        .await
        .expect_err("no period");
    assert!(
        err.error.contains("missing 'time_dimension'"),
        "{}",
        err.error
    );
    assert!(err.hint.unwrap().contains("`list_anomalies` schema"));
}
