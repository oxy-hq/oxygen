//! The react runtime on the headless path, driven by a scripted engine so no
//! test calls a model. `query` is the real tool over an in-memory connector.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use agentic_analytics::AnalyticsEvent;
use agentic_analytics::config::{AgentConfig, AgentRuntime};
use agentic_analytics::react::WorkspaceFiles;
use agentic_analytics::react::run::{ReactAnalytics, ReactInputs};
use agentic_connector::{
    ConnectorError, DatabaseConnector, ExecutionResult, ResultSummary, SqlDialect,
};
use agentic_core::events::{CoreEvent, Event};
use agentic_core::react::{
    Decision, Engine, EngineError, EventSink, ToolCallReq, Transcript, Turn, TurnUsage,
};
use agentic_core::result::{CellValue, QueryResult, QueryRow};
use agentic_core::tools::ToolDef;
use agentic_runtime::react::{LoopOutcome, ResultStore};
use serde_json::{Value, json};

use super::*;
use crate::headless::{EventDestination, HeadlessError};

/// Answers with its script, in order.
pub(crate) struct Scripted(Mutex<VecDeque<Decision>>);

#[async_trait::async_trait]
impl Engine for Scripted {
    async fn decide(
        &self,
        _: &Transcript,
        _: &[ToolDef],
        _: &EventSink,
    ) -> Result<Decision, EngineError> {
        let next = self.0.lock().unwrap().pop_front();
        Ok(next.expect("scripted engine ran past its script"))
    }
}

/// One table, one row: `n = 5`.
pub(crate) struct Five;

#[async_trait::async_trait]
impl DatabaseConnector for Five {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::Postgres
    }

    async fn execute_query(&self, _: &str, _: u64) -> Result<ExecutionResult, ConnectorError> {
        Err(ConnectorError::Other(
            "react queries take the read-only path".into(),
        ))
    }

    fn supports_read_only(&self) -> bool {
        true
    }

    async fn execute_query_read_only(
        &self,
        _: &str,
        _: u64,
    ) -> Result<ExecutionResult, ConnectorError> {
        Ok(ExecutionResult {
            result: QueryResult {
                columns: vec!["n".into()],
                rows: vec![QueryRow(vec![CellValue::Number(5.0)])],
                total_row_count: 1,
                truncated: false,
            },
            summary: ResultSummary {
                row_count: 1,
                columns: Vec::new(),
            },
        })
    }
}

/// A connector with no read-only path, as BigQuery is today.
pub(crate) struct NoBackstop;

#[async_trait::async_trait]
impl DatabaseConnector for NoBackstop {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::BigQuery
    }

    async fn execute_query(&self, _: &str, _: u64) -> Result<ExecutionResult, ConnectorError> {
        Ok(ExecutionResult::empty())
    }
}

/// A platform the build never reaches past its first checks.
pub(crate) struct NoPlatform;

#[async_trait::async_trait]
impl crate::platform::ProjectContext for NoPlatform {
    async fn resolve_agent(
        &self,
        _: &str,
    ) -> Result<Option<String>, agentic_automation::WorkspaceReadError> {
        Ok(None)
    }
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
}

#[async_trait::async_trait]
impl agentic_automation::WorkspaceContext for NoPlatform {
    fn workspace_path(&self) -> Option<&std::path::Path> {
        None
    }
    fn database_configs(&self) -> Vec<oxy_airlayer_compat::DatabaseConfig> {
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

pub(crate) fn turn(text: &str, calls: Vec<ToolCallReq>) -> Turn {
    Turn {
        text: text.into(),
        tool_calls: calls,
        native: Value::Null,
        usage: TurnUsage::default(),
    }
}

pub(crate) fn call(name: &str, input: Value) -> Decision {
    let req = ToolCallReq {
        id: "c1".into(),
        name: name.into(),
        input,
    };
    Decision::CallTools(turn("", vec![req]))
}

pub(crate) fn react_run(script: Vec<Decision>, dir: &std::path::Path) -> ReactRun {
    let files = Arc::new(WorkspaceFiles::load(dir, &[], None).unwrap());
    let databases: HashMap<String, Arc<dyn DatabaseConnector>> = HashMap::from([(
        "wh".to_string(),
        Arc::new(Five) as Arc<dyn DatabaseConnector>,
    )]);
    let analytics = ReactAnalytics::new(ReactInputs {
        instructions: String::new(),
        databases,
        catalog: None,
        default_connector: None,
        files,
        semantic_only: false,
        advisor: false,
    });
    ReactRun {
        engine: Box::new(Scripted(Mutex::new(script.into()))),
        system: analytics.toolset.system_prompt.clone(),
        analytics,
        results: Arc::new(ResultStore::new()),
        after_done: None,
    }
}

async fn run_forwarding(
    script: Vec<Decision>,
) -> (Result<String, HeadlessError>, Vec<Event<AnalyticsEvent>>) {
    let dir = tempfile::tempdir().unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    let destination = EventDestination::Forward(tx);
    let result = run_react_headless(react_run(script, dir.path()), "q".into(), destination).await;
    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }
    (result, events)
}

#[test]
fn row_to_event_maps_core_and_domain_rows() {
    let token = row_to_event("llm_token", &json!({"token": "a"}));
    assert!(matches!(
        token,
        Some(Event::Core(CoreEvent::LlmToken { ref token, .. })) if token == "a"
    ));
    let query = json!({
        "query": "select 5", "row_count": 1, "duration_ms": 3, "success": true,
        "error": null, "columns": ["n"], "rows": [[5]], "source": "llm", "is_preagg": false,
    });
    assert!(matches!(
        row_to_event("query_executed", &query),
        Some(Event::Domain(AnalyticsEvent::QueryExecuted { .. }))
    ));
    assert!(row_to_event("react_turn", &json!({"text": "hi"})).is_none());
    // A drifted payload parses as neither; it is dropped (and logged).
    assert!(row_to_event("query_executed", &json!({"query": 1})).is_none());
}

#[tokio::test]
async fn headless_react_run_returns_the_stripped_answer() {
    let script = vec![
        call("query", json!({"sql": "select 5 as n"})),
        Decision::Reply(turn("5 {{result:r1|primary}}", vec![])),
    ];
    let (result, events) = run_forwarding(script).await;
    assert_eq!(result.unwrap(), "5");
    let domain: Vec<&AnalyticsEvent> = events
        .iter()
        .filter_map(|e| match e {
            Event::Domain(d) => Some(d),
            Event::Core(_) => None,
        })
        .collect();
    assert!(matches!(
        domain.first(),
        Some(AnalyticsEvent::RuntimeSelected { runtime }) if runtime == "react"
    ));
    assert!(
        domain
            .iter()
            .any(|d| matches!(d, AnalyticsEvent::QueryExecuted { success: true, .. }))
    );
    assert!(domain.iter().any(|d| matches!(
        d,
        AnalyticsEvent::AnswerResult { result_id, .. } if result_id == "r1"
    )));
    let at = |f: &dyn Fn(&Event<AnalyticsEvent>) -> bool| events.iter().position(|e| f(e));
    let answer_at = at(&|e| matches!(e, Event::Domain(AnalyticsEvent::AnswerResult { .. })));
    let done_at = at(&|e| matches!(e, Event::Core(CoreEvent::Done { .. })));
    assert!(
        answer_at.unwrap() < done_at.unwrap(),
        "answer_result precedes done"
    );
    let dones = events
        .iter()
        .filter(|e| matches!(e, Event::Core(CoreEvent::Done { .. })))
        .count();
    assert_eq!(dones, 1, "a successful run forwards exactly one done");
}

#[tokio::test]
async fn headless_react_suspend_fails_like_the_fsm() {
    let script = vec![call("ask_user", json!({"question": "Which year?"}))];
    let (result, events) = run_forwarding(script).await;
    assert!(
        !events.iter().any(|e| matches!(
            e,
            Event::Core(CoreEvent::Done { .. } | CoreEvent::Error { .. })
        )),
        "a suspend writes neither done nor error"
    );
    match result {
        Err(HeadlessError::Run(m)) => {
            assert_eq!(
                m,
                "agentic pipeline asked a clarifying question: Which year?"
            )
        }
        other => panic!("expected a run error, got {other:?}"),
    }
}

#[test]
fn headless_fsm_agent_is_unchanged() {
    let fsm = AgentConfig::from_yaml("instructions: hi\n").unwrap();
    assert_eq!(fsm.runtime, AgentRuntime::Fsm);
    assert!(
        !runs_react(&fsm),
        "an agent with no `runtime:` key stays on the FSM"
    );
    let react = AgentConfig::from_yaml("runtime: react\n").unwrap();
    assert!(runs_react(&react));
}

/// Until PR B's per-org `react_runtime` flag exists, only the eval harness
/// entry may honor `runtime: react`: Slack and the health probe run the FSM.
#[test]
fn react_opt_in_is_ignored_on_the_slack_and_health_probe_entries() {
    use super::{EVAL_ENTRY_REACT, STREAMING_ENTRY_REACT};
    let react = AgentConfig::from_yaml("runtime: react\n").unwrap();
    assert_eq!(STREAMING_ENTRY_REACT, ReactPolicy::Never);
    assert_eq!(EVAL_ENTRY_REACT, ReactPolicy::Never);
    assert!(!picks_react(&react, STREAMING_ENTRY_REACT, "a.agentic.yml"));
    assert!(!picks_react(&react, EVAL_ENTRY_REACT, "a.agentic.yml"));
}

#[test]
fn react_opt_in_is_honored_on_the_eval_harness_entry() {
    use super::HARNESS_ENTRY_REACT;
    let react = AgentConfig::from_yaml("runtime: react\n").unwrap();
    assert_eq!(HARNESS_ENTRY_REACT, ReactPolicy::Allow);
    assert!(picks_react(&react, HARNESS_ENTRY_REACT, "a.agentic.yml"));
    let fsm = AgentConfig::from_yaml("instructions: hi\n").unwrap();
    assert!(
        !picks_react(&fsm, ReactPolicy::Allow, "a.agentic.yml"),
        "Allow honors an opt-in; it never makes an FSM agent react"
    );
}

#[test]
fn headless_result_maps_failed_refused_and_cancelled_to_run_errors() {
    let text = |o: LoopOutcome| match headless_result(o) {
        Err(HeadlessError::Run(m)) => m,
        other => panic!("expected a run error, got {other:?}"),
    };
    let failed = LoopOutcome::Failed {
        message: "budget exceeded".into(),
    };
    assert_eq!(text(failed), "budget exceeded");
    let refused = LoopOutcome::Refused {
        reason: "stale resume".into(),
    };
    assert_eq!(text(refused), "stale resume");
    assert_eq!(text(LoopOutcome::Cancelled), "cancelled");
}
