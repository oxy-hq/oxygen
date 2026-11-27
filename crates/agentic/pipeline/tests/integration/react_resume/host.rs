//! The fixtures `react_resume` drives: a scripted model, a read-only
//! warehouse, the host platform that hands both to the pipeline, the queued
//! chat's driver, and one latency-worker pass over them.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use agentic_connector::{
    ConnectorError, DatabaseConnector, ExecutionResult, ResultSummary, SqlDialect,
};
use agentic_core::delegation::TaskAssignment;
use agentic_core::react::{
    Decision, Engine, EngineError, EventSink, ToolCallReq, Transcript, Turn, TurnUsage,
};
use agentic_core::result::{CellValue, QueryResult, QueryRow};
use agentic_core::tools::ToolDef;
use agentic_pipeline::platform::{IdentityResolver, PlatformContext};
use agentic_pipeline::queued_run::{CHAT_RUN_KIND, ChatStart};
use agentic_pipeline::recovery::DrivePolicy;
use agentic_runtime::crud;
use agentic_runtime::state::RuntimeState;
use agentic_runtime::worker::{CustomTaskRegistry, ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};
use uuid::Uuid;

// ── The scripted model ──────────────────────────────────────────────────────

/// Answers with a script shared by every engine a host builds, and records
/// each transcript it is asked to decide on. Past its script it fails, so an
/// unexpected model call is a visible failure, never a live call.
#[derive(Clone, Default)]
pub(super) struct Script {
    decisions: Arc<Mutex<VecDeque<Decision>>>,
    seen: Arc<Mutex<Vec<Transcript>>>,
}

#[async_trait]
impl Engine for Script {
    async fn decide(
        &self,
        transcript: &Transcript,
        _: &[ToolDef],
        _: &EventSink,
    ) -> Result<Decision, EngineError> {
        self.seen.lock().unwrap().push(transcript.clone());
        let next = self.decisions.lock().unwrap().pop_front();
        next.ok_or_else(|| EngineError::Fatal("the scripted engine ran past its script".into()))
    }
}

pub(super) fn turn(text: &str, calls: Vec<ToolCallReq>) -> Turn {
    Turn {
        text: text.into(),
        tool_calls: calls,
        native: Value::Null,
        usage: TurnUsage::default(),
    }
}

pub(super) fn ask_user() -> Decision {
    let call = ToolCallReq {
        id: "c1".into(),
        name: "ask_user".into(),
        input: json!({ "question": "Which year?", "suggestions": ["2024", "2025"] }),
    };
    Decision::CallTools(turn("", vec![call]))
}

/// A warehouse with a database-level read-only path, as react requires.
struct ReadOnly;

#[async_trait]
impl DatabaseConnector for ReadOnly {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::Postgres
    }

    async fn execute_query(&self, _: &str, _: u64) -> Result<ExecutionResult, ConnectorError> {
        Err(ConnectorError::Other(
            "react takes the read-only path".into(),
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

/// A warehouse connection with no read-only path, as an `airhouse_managed`
/// one is until the platform swaps in a read-only credential.
struct Writable;

#[async_trait]
impl DatabaseConnector for Writable {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::DuckDb
    }

    async fn execute_query(&self, _: &str, _: u64) -> Result<ExecutionResult, ConnectorError> {
        Err(ConnectorError::Other(
            "react must query the read-only replacement".into(),
        ))
    }
}

// ── The host ────────────────────────────────────────────────────────────────

/// One process's platform: the agent (`agent`: [`REACT_AGENT`] unless a test
/// says otherwise), the org's `react_runtime` flag, and the scripted engine every react build
/// on it gets. `engines_built` counts react builds; `fsm_params` counts the
/// FSM's pipeline params being assembled (the one caller of `metric_sink`
/// on a resume).
pub(super) struct Host {
    pub dir: tempfile::TempDir,
    pub flag: bool,
    pub warehouse: bool,
    /// The connector `warehouse` resolves to; `None`: the scripted
    /// [`ReadOnly`] one.
    pub connector: Option<Arc<dyn DatabaseConnector>>,
    /// The warehouse is like an `airhouse_managed` one: the pre-built
    /// connector has no read-only path ([`Writable`]), and only
    /// `read_only_connector` gives one ([`ReadOnly`]).
    pub airhouse_like: bool,
    /// Where served runs store results (`PlatformContext::result_archive`);
    /// `None` stores nothing.
    pub archive: Option<Arc<dyn agentic_runtime::react::ResultArchive>>,
    pub agent: &'static str,
    pub script: Script,
    pub engines_built: AtomicUsize,
    pub fsm_params: AtomicUsize,
}

/// The agent a react run starts on: it opts in, over one read-only warehouse.
pub(super) const REACT_AGENT: &str = "runtime: react\ndatabases: [warehouse]\n";

impl Host {
    pub(super) fn new(flag: bool, script: Vec<Decision>) -> Arc<Self> {
        Arc::new(Self::build(flag, script))
    }

    pub(super) fn build(flag: bool, script: Vec<Decision>) -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            flag,
            warehouse: true,
            connector: None,
            airhouse_like: false,
            archive: None,
            agent: REACT_AGENT,
            script: Script {
                decisions: Arc::new(Mutex::new(script.into())),
                ..Script::default()
            },
            engines_built: AtomicUsize::new(0),
            fsm_params: AtomicUsize::new(0),
        }
    }

    pub(super) fn seen(&self) -> Vec<Transcript> {
        self.script.seen.lock().unwrap().clone()
    }
}

#[async_trait]
impl agentic_pipeline::platform::ProjectContext for Host {
    async fn resolve_connector(&self, _: &str) -> Option<agentic_connector::ConnectorConfig> {
        None
    }

    async fn resolve_pre_built_connector(&self, name: &str) -> Option<Arc<dyn DatabaseConnector>> {
        if !self.warehouse || name != "warehouse" {
            return None;
        }
        if self.airhouse_like {
            return Some(Arc::new(Writable));
        }
        Some(self.connector.clone().unwrap_or_else(|| Arc::new(ReadOnly)))
    }

    fn result_archive(&self) -> Option<Arc<dyn agentic_runtime::react::ResultArchive>> {
        self.archive.clone()
    }

    async fn read_only_connector(&self, name: &str) -> Option<Arc<dyn DatabaseConnector>> {
        (self.airhouse_like && name == "warehouse").then(|| Arc::new(ReadOnly) as _)
    }

    /// A model at an address nothing listens on: a run that falls back to
    /// the FSM fails its first model call at once, and no test can reach a
    /// live model whatever key the environment holds. React runs use the
    /// script ([`Self::react_engine`]).
    async fn resolve_model(
        &self,
        _: Option<&str>,
        _: bool,
    ) -> Option<agentic_analytics::config::ResolvedModelInfo> {
        Some(agentic_analytics::config::ResolvedModelInfo {
            model: "test-model".into(),
            vendor: agentic_analytics::config::LlmVendor::Anthropic,
            api_key: Some("test-key".into()),
            base_url: Some("http://127.0.0.1:9".into()),
            azure_deployment_id: None,
            azure_api_version: None,
            headers: None,
        })
    }

    async fn resolve_agent(
        &self,
        _: &str,
    ) -> Result<Option<String>, agentic_pipeline::WorkspaceReadError> {
        Ok(Some(self.agent.into()))
    }

    async fn resolve_secret(&self, _: &str) -> Option<String> {
        None
    }

    async fn react_runtime_enabled(&self) -> bool {
        self.flag
    }

    fn react_engine(&self) -> Option<Box<dyn Engine>> {
        self.engines_built.fetch_add(1, Ordering::SeqCst);
        Some(Box::new(self.script.clone()))
    }

    fn metric_sink(&self) -> Option<agentic_analytics::SharedMetricSink> {
        self.fsm_params.fetch_add(1, Ordering::SeqCst);
        None
    }
}

#[async_trait]
impl agentic_automation::WorkspaceContext for Host {
    fn workspace_path(&self) -> Option<&std::path::Path> {
        Some(self.dir.path())
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

    async fn resolve_automation_yaml(
        &self,
        _: &str,
    ) -> Result<String, agentic_pipeline::WorkspaceReadError> {
        Err("unused".into())
    }
}

/// The queued chat's driver, as the host's `ChatRunExecutor` drives one: the
/// chat the run row describes, on the run the queue names.
struct ChatRun {
    db: DatabaseConnection,
    host: Arc<Host>,
}

#[async_trait]
impl TaskExecutor for ChatRun {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let run = crud::get_run(&self.db, &assignment.run_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("no run")?;
        let chat = ChatStart::from_run(&run)?;
        let started = chat
            .builder(self.host.clone())
            .existing_run(assignment.run_id.clone())
            .start(&self.db)
            .await
            .map_err(|e| e.to_string())?;
        Ok(started.into_executing_task().0)
    }
}

/// One latency-worker pass over `ws` on `host`, as a freshly started process
/// runs it: its own runtime state, executor and registry.
pub(super) async fn latency_pass(db: &DatabaseConnection, ws: Uuid, host: &Arc<Host>) {
    let mut registry = CustomTaskRegistry::new();
    let chat = ChatRun {
        db: db.clone(),
        host: host.clone(),
    };
    registry.register(CHAT_RUN_KIND, Arc::new(chat));
    let platform: Arc<dyn PlatformContext> = host.clone();
    agentic_pipeline::recovery::recover_pending_global_runs(
        db.clone(),
        Arc::new(RuntimeState::new()),
        platform,
        Arc::new(IdentityResolver),
        agentic_runtime::router::noop_router(),
        Some(ws),
        Some(Arc::new(registry)),
        DrivePolicy::ALL,
    )
    .await;
}
