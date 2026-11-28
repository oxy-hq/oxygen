//! The metric-tree tools on both react hosts: a served build and a headless
//! run offer them exactly when the platform has a metric-tree runner, and a
//! headless call reaches the runner. A scripted engine stands in for the
//! model. The host here also serves the anomaly tests
//! (`react_anomaly_tests.rs`), which give it a store.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use agentic_analytics::anomaly_store::AnomalyStore;
use agentic_analytics::config::{AgentConfig, BuildContext};
use agentic_analytics::{AnalyticsEvent, MetricTreeRunner, MetricTreeRunnerError};
use agentic_connector::DatabaseConnector;
use agentic_core::events::{CoreEvent, Event};
use agentic_core::react::{Decision, Engine, EngineError, EventSink, Transcript};
use agentic_core::tools::ToolDef;
use oxy_airlayer_compat::engine::metric_tree_ops::{
    BenchmarkStatistic, ExplainConfig, ExplainResult, OpportunityResult,
};
use oxy_airlayer_compat::engine::query::QueryFilter;
use oxy_airlayer_compat::{DatabaseConfig, SemanticLayer};
use serde_json::json;

use super::tests::{Five, call, turn};
use super::{EvalMemory, ReactRun, run_headless};
use crate::ThinkingMode;
use crate::headless::EventDestination;
use crate::react_served::{ServedInputs, build_served};

const TREE_TOOLS: [&str; 4] = [
    "explain_metric",
    "find_opportunities",
    "metric_sensitivity",
    "predict_impact",
];

/// Hands back a layer with `orders.profit = orders.revenue - orders.cost`;
/// `predict_impact` is a pure op over it, so nothing here queries.
struct ProfitTree;

fn unused() -> MetricTreeRunnerError {
    MetricTreeRunnerError::Op("not called in these tests".into())
}

#[async_trait::async_trait]
impl MetricTreeRunner for ProfitTree {
    async fn load_layer(&self) -> Result<SemanticLayer, MetricTreeRunnerError> {
        let view = oxy_airlayer_compat::parse_view_yaml(
            "name: orders\ntable: public.orders\ndialect: postgres\nmeasures:\n  \
             - name: revenue\n    type: sum\n    expr: amount\n  \
             - name: cost\n    type: sum\n    expr: cost\n  \
             - name: profit\n    type: number\n    expr: \"{{orders.revenue}} - {{orders.cost}}\"\n",
        )
        .map_err(|e| MetricTreeRunnerError::LayerLoad(e.to_string()))?;
        Ok(SemanticLayer::new(vec![view], None))
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

/// Answers with its script, in order, and records the tool names it was
/// offered on its first turn.
struct Recording {
    script: Mutex<VecDeque<Decision>>,
    offered: Arc<Mutex<Vec<&'static str>>>,
}

#[async_trait::async_trait]
impl Engine for Recording {
    async fn decide(
        &self,
        _: &Transcript,
        tools: &[ToolDef],
        _: &EventSink,
    ) -> Result<Decision, EngineError> {
        let mut offered = self.offered.lock().unwrap();
        if offered.is_empty() {
            offered.extend(tools.iter().map(|t| t.name));
        }
        let next = self.script.lock().unwrap().pop_front();
        Ok(next.expect("scripted engine ran past its script"))
    }
}

/// The workspace a [`TreeHost`] answers for.
pub(super) const WORKSPACE: uuid::Uuid = uuid::Uuid::from_u128(0x7ee);

/// A platform with (or without) a metric-tree runner and an anomaly store,
/// whose react engine is a [`Recording`] of `script`.
pub(super) struct TreeHost {
    runner: Option<Arc<dyn MetricTreeRunner>>,
    store: Option<Arc<dyn AnomalyStore>>,
    script: Mutex<Vec<Decision>>,
    offered: Arc<Mutex<Vec<&'static str>>>,
}

impl TreeHost {
    pub(super) fn new(with_runner: bool, script: Vec<Decision>) -> Self {
        Self {
            runner: with_runner.then(|| Arc::new(ProfitTree) as Arc<dyn MetricTreeRunner>),
            store: None,
            script: Mutex::new(script),
            offered: Arc::default(),
        }
    }

    /// This host with `store` as its anomaly store.
    pub(super) fn with_store(mut self, store: Arc<dyn AnomalyStore>) -> Self {
        self.store = Some(store);
        self
    }

    /// Every tool name the engine was offered on its first turn.
    pub(super) fn offered(&self) -> Vec<&'static str> {
        self.offered.lock().unwrap().clone()
    }

    /// The metric-tree tools the engine was offered.
    fn offered_tree_tools(&self) -> Vec<&'static str> {
        let offered = self.offered.lock().unwrap();
        offered
            .iter()
            .copied()
            .filter(|n| TREE_TOOLS.contains(n))
            .collect()
    }
}

#[async_trait::async_trait]
impl crate::platform::ProjectContext for TreeHost {
    async fn resolve_connector(&self, _: &str) -> Option<agentic_connector::ConnectorConfig> {
        None
    }
    async fn resolve_model(
        &self,
        _: Option<&str>,
        _: bool,
    ) -> Option<agentic_analytics::config::ResolvedModelInfo> {
        None
    }
    async fn resolve_secret(&self, _: &str) -> Option<String> {
        None
    }
    fn workspace_id(&self) -> uuid::Uuid {
        WORKSPACE
    }
    fn metric_tree_runner(&self) -> Option<Arc<dyn MetricTreeRunner>> {
        self.runner.clone()
    }
    fn anomaly_store(&self) -> Option<Arc<dyn AnomalyStore>> {
        self.store.clone()
    }
    fn react_engine(&self) -> Option<Box<dyn Engine>> {
        let script = std::mem::take(&mut *self.script.lock().unwrap());
        Some(Box::new(Recording {
            script: Mutex::new(script.into()),
            offered: self.offered.clone(),
        }))
    }
}

#[async_trait::async_trait]
impl agentic_automation::WorkspaceContext for TreeHost {
    fn workspace_path(&self) -> Option<&std::path::Path> {
        None
    }
    fn database_configs(&self) -> Vec<DatabaseConfig> {
        vec![]
    }
    async fn get_connector(&self, name: &str) -> Result<Arc<dyn DatabaseConnector>, String> {
        Err(format!("no connector {name}"))
    }
    async fn get_integration(
        &self,
        name: &str,
    ) -> Result<agentic_automation::workspace::IntegrationConfig, String> {
        Err(format!("no integration {name}"))
    }
    async fn list_automation_files(&self) -> Result<Vec<std::path::PathBuf>, String> {
        Ok(vec![])
    }
    async fn resolve_automation_yaml(&self, _: &str) -> Result<String, crate::WorkspaceReadError> {
        Err("unused".into())
    }
}

fn warehouse() -> HashMap<String, Arc<dyn DatabaseConnector>> {
    HashMap::from([(
        "wh".to_string(),
        Arc::new(Five) as Arc<dyn DatabaseConnector>,
    )])
}

fn react_config() -> AgentConfig {
    AgentConfig::from_yaml("runtime: react\n").unwrap()
}

fn tree_tools_in(run: &ReactRun) -> Vec<&'static str> {
    let tools = run.analytics.toolset.tools.iter().map(|t| t.def().name);
    tools.filter(|n| TREE_TOOLS.contains(n)).collect()
}

/// The run a served build (start and resume both build here) makes on `host`.
pub(super) async fn served(host: TreeHost) -> ReactRun {
    let dir = tempfile::tempdir().unwrap();
    let inputs = ServedInputs {
        agent_id: "a".into(),
        workspace_id: uuid::Uuid::nil(),
        connectors: warehouse(),
        databases: vec!["wh".into()],
        project_model: None,
        domain_docs: None,
        thinking_mode: ThinkingMode::Auto,
    };
    // Memory is off on this host, so the build never reads the database.
    let db = sea_orm::DatabaseConnection::default();
    build_served(&host, &db, &react_config(), inputs, dir.path())
        .await
        .expect("the run builds")
}

/// A served build offers the four tools, and says when to use them, exactly
/// when the platform has a runner.
#[tokio::test]
async fn a_served_react_run_gets_the_tree_tools_only_with_a_runner() {
    let with = served(TreeHost::new(true, vec![])).await;
    assert_eq!(tree_tools_in(&with), TREE_TOOLS);
    assert!(with.system.contains("explain_metric"), "{}", with.system);

    let without = served(TreeHost::new(false, vec![])).await;
    assert!(tree_tools_in(&without).is_empty());
    assert!(
        !without.system.contains("explain_metric"),
        "{}",
        without.system
    );
}

/// Runs `script` headless on a host with or without a runner, returning the
/// answer and every forwarded event.
async fn headless(with_runner: bool, script: Vec<Decision>) -> Headless {
    let host = TreeHost::new(with_runner, script);
    let (answer, events) = headless_on(&host).await;
    Headless {
        answer,
        events,
        offered: host.offered_tree_tools(),
    }
}

/// Runs `host`'s script headless (evals, the CLI), returning the answer and
/// every forwarded event.
pub(super) async fn headless_on(
    host: &TreeHost,
) -> (
    Result<String, crate::headless::HeadlessError>,
    Vec<Event<AnalyticsEvent>>,
) {
    let dir = tempfile::tempdir().unwrap();
    let ctx = BuildContext {
        extra_connectors: warehouse(),
        ..BuildContext::default()
    };
    let memory = EvalMemory {
        read: None,
        write: None,
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    let run = ("a.agentic.yml", dir.path(), &memory);
    let destination = EventDestination::Forward(tx);
    let answer = run_headless(host, &react_config(), ctx, run, "q".into(), destination).await;
    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }
    (answer, events)
}

/// What a headless run returned, forwarded, and offered the model.
struct Headless {
    answer: Result<String, crate::headless::HeadlessError>,
    events: Vec<Event<AnalyticsEvent>>,
    offered: Vec<&'static str>,
}

pub(super) fn tool_calls(events: &[Event<AnalyticsEvent>]) -> Vec<&str> {
    let names = events.iter().filter_map(|e| match e {
        Event::Core(CoreEvent::ToolCall { name, .. }) => Some(name.as_str()),
        _ => None,
    });
    names.collect()
}

/// A headless run (evals, the CLI) calls `predict_impact` through the
/// platform's runner, and the call is the `tool_call` an eval's
/// `expect.tools` reads.
#[tokio::test]
async fn a_headless_react_run_calls_a_tree_tool_through_the_runner() {
    let args = json!({"changes": [{"measure": "orders.revenue", "delta": 100.0}]});
    let script = vec![
        call("predict_impact", args),
        Decision::Reply(turn("Profit rises by 100.", vec![])),
    ];
    let run = headless(true, script).await;
    assert_eq!(run.offered, TREE_TOOLS);
    assert_eq!(run.answer.unwrap(), "Profit rises by 100.");
    assert_eq!(tool_calls(&run.events), vec!["predict_impact"]);
}

/// With no runner the model is offered none of the four.
#[tokio::test]
async fn a_headless_react_run_without_a_runner_has_no_tree_tools() {
    let script = vec![Decision::Reply(turn("No tree here.", vec![]))];
    let run = headless(false, script).await;
    assert!(run.offered.is_empty(), "{:?}", run.offered);
    assert_eq!(run.answer.unwrap(), "No tree here.");
}
