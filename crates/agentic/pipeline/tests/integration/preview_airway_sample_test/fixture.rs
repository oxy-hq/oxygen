//! A platform that is (or is not) a workspace preview, serving one pipeline and
//! recording every secret it is asked for; and the runs the tests compare.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_pipeline::airway_preview::PreviewSample;
use agentic_pipeline::executor::PipelineTaskExecutor;
use agentic_pipeline::platform::{
    AirwayStepMode, PlatformContext, PreviewScope, ProjectContext, ResolvedPipelineDestination,
};
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::Value;
use uuid::Uuid;

pub(super) const KEY: &str = "feat_qb_v2_92a1b7";
pub(super) const REF: &str = "pipelines/p.airway.yml";

pub(super) struct SamplePlatform {
    pub ws: Uuid,
    pub scope: Option<PreviewScope>,
    pub yaml: String,
    /// What `resolve_secret` answers; every name asked is recorded.
    pub secrets: HashMap<String, String>,
    pub asked: Mutex<Vec<String>>,
    /// The one var `persist_secret` may write; any other panics.
    pub rotating: Option<String>,
    pub destination: Option<ResolvedPipelineDestination>,
}

impl SamplePlatform {
    pub fn new(ws: Uuid, scope: Option<PreviewScope>, yaml: String) -> Self {
        Self {
            ws,
            scope,
            yaml,
            secrets: HashMap::new(),
            asked: Mutex::new(Vec::new()),
            rotating: None,
            destination: None,
        }
    }
}

pub(super) fn scope(run_id: &str) -> PreviewScope {
    PreviewScope {
        run_id: run_id.into(),
        preview_key: KEY.into(),
        revision_id: Uuid::nil(),
        airway_steps: AirwayStepMode::Hold,
    }
}

/// A sample of the `users` pipeline: no window, its one resource named (what
/// submit stores for a single-resource source).
pub(super) fn sample() -> PreviewSample {
    PreviewSample {
        preview_key: KEY.into(),
        window: None,
        resources: vec!["users".into()],
        sandbox: None,
    }
}

#[async_trait]
impl ProjectContext for SamplePlatform {
    async fn resolve_connector(&self, _db: &str) -> Option<agentic_connector::ConnectorConfig> {
        None
    }
    async fn resolve_model(
        &self,
        _model_ref: Option<&str>,
        _has_explicit_model: bool,
    ) -> Option<agentic_analytics::config::ResolvedModelInfo> {
        None
    }
    async fn resolve_secret(&self, var_name: &str) -> Option<String> {
        self.asked.lock().unwrap().push(var_name.to_string());
        self.secrets.get(var_name).cloned()
    }
    async fn persist_secret(&self, var_name: &str, _value: &str) -> Result<(), String> {
        assert_eq!(
            Some(var_name),
            self.rotating.as_deref(),
            "a sample persisted a var other than its sandbox's rotating one"
        );
        Ok(())
    }
    async fn resolve_pipeline_destination(
        &self,
        _db_name: &str,
        _dataset_name: &str,
    ) -> Option<ResolvedPipelineDestination> {
        self.destination.clone()
    }
    fn workspace_id(&self) -> Uuid {
        self.ws
    }
    fn preview_scope(&self) -> Option<PreviewScope> {
        self.scope.clone()
    }
}

#[async_trait]
impl agentic_automation::WorkspaceContext for SamplePlatform {
    fn workspace_path(&self) -> Option<&Path> {
        None
    }
    fn database_configs(&self) -> Vec<oxy_airlayer_compat::DatabaseConfig> {
        vec![]
    }
    async fn get_connector(
        &self,
        name: &str,
    ) -> Result<Arc<dyn agentic_connector::DatabaseConnector>, String> {
        Err(format!("no connector {name}"))
    }
    async fn get_integration(
        &self,
        name: &str,
    ) -> Result<agentic_automation::workspace::IntegrationConfig, String> {
        Err(format!("no integration {name}"))
    }
    async fn list_automation_files(&self) -> Result<Vec<PathBuf>, String> {
        Ok(vec![])
    }
    async fn resolve_automation_yaml(
        &self,
        _r: &str,
    ) -> Result<String, agentic_pipeline::WorkspaceReadError> {
        Err("unused".into())
    }
    async fn resolve_pipeline_yaml(&self, _r: &str) -> Result<Option<String>, String> {
        Ok(Some(self.yaml.clone()))
    }
}

pub(super) fn executor(db: &DatabaseConnection, platform: SamplePlatform) -> PipelineTaskExecutor {
    let platform: Arc<dyn PlatformContext> = Arc::new(platform);
    PipelineTaskExecutor::bare(platform, db.clone())
}

/// A directory holding three user rows, and a filesystem → memory pipeline
/// named `name` over it.
pub(super) fn users(name: &str) -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut f = std::fs::File::create(dir.path().join("users.jsonl")).unwrap();
    for (id, who) in [(1, "Alice"), (2, "Bob"), (3, "Carol")] {
        writeln!(f, r#"{{"id": {id}, "name": "{who}"}}"#).unwrap();
    }
    let yaml = format!(
        "name: {name}
source:
  kind: filesystem
  config:
    base_path: {base}
    pattern: \"*.jsonl\"
    format: jsonl
    table_name: users
destination:
  kind: memory
  config:
    dataset_name: scratch
",
        base = dir.path().display()
    );
    (dir, yaml)
}

pub(super) async fn seed_run(db: &DatabaseConnection, run_id: &str, source_type: &str, ws: Uuid) {
    agentic_runtime::crud::insert_run(db, run_id, "Q", None, source_type, None, ws)
        .await
        .expect("seed run");
}

/// Drain `task` to its outcome, with every event type it emitted.
pub(super) async fn drive(mut task: ExecutingTask) -> (TaskOutcome, Vec<(String, Value)>) {
    let mut events = Vec::new();
    let outcome = loop {
        tokio::select! {
            ev = task.events.recv() => if let Some(ev) = ev { events.push(ev) },
            oc = task.outcomes.recv() => break oc.expect("an outcome"),
            _ = tokio::time::sleep(Duration::from_secs(60)) => panic!("timed out: {events:?}"),
        }
    };
    while let Ok(Some(ev)) =
        tokio::time::timeout(Duration::from_millis(100), task.events.recv()).await
    {
        events.push(ev);
    }
    (outcome, events)
}

/// Production's own run of `yaml` in `ws`, through the ordinary `TaskSpec::Airway` path.
pub(super) async fn run_production(db: &DatabaseConnection, ws: Uuid, yaml: &str) -> TaskOutcome {
    let run_id = Uuid::new_v4().to_string();
    seed_run(db, &run_id, "airway", ws).await;
    let exec = executor(db, SamplePlatform::new(ws, None, yaml.to_string()));
    let task = exec
        .execute(TaskAssignment {
            task_id: run_id.clone(),
            parent_task_id: None,
            run_id,
            spec: TaskSpec::Airway {
                pipeline_ref: REF.into(),
                variables: None,
                resources: vec![],
                backfill_from: None,
                backfill_to: None,
                contract_policy: None,
                environment: None,
            },
            policy: None,
        })
        .await
        .unwrap_or_else(|e| panic!("production dispatch: {e}"));
    drive(task).await.0
}

/// A sample of `yaml` in `ws` on a preview platform. Its run id, outcome and events.
pub(super) async fn run_sample(
    db: &DatabaseConnection,
    ws: Uuid,
    yaml: &str,
) -> (String, TaskOutcome, Vec<(String, Value)>) {
    let run_id = Uuid::new_v4().to_string();
    seed_run(db, &run_id, agentic_pipeline::PREVIEW_AIRWAY_SAMPLE, ws).await;
    let platform = SamplePlatform::new(ws, Some(scope(&run_id)), yaml.to_string());
    let task = executor(db, platform)
        .execute_airway_preview_sample(&run_id, REF, &sample())
        .await
        .unwrap_or_else(|e| panic!("sample dispatch: {e}"));
    let (outcome, events) = drive(task).await;
    (run_id, outcome, events)
}

/// A table's row as Postgres' own JSON text: byte-comparable.
pub(super) async fn row_text(
    db: &DatabaseConnection,
    sql: &str,
    values: Vec<sea_orm::Value>,
) -> Vec<String> {
    db.query_all_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .await
    .expect(sql)
    .iter()
    .map(|r| r.try_get::<String>("", "t").unwrap())
    .collect()
}

pub(super) async fn state_rows(db: &DatabaseConnection, ws: Uuid, name: &str) -> Vec<String> {
    row_text(
        db,
        "SELECT row_to_json(s)::text AS t FROM airway_workspace_pipeline_state s \
         WHERE workspace_id = $1 AND pipeline_name = $2",
        vec![ws.into(), name.into()],
    )
    .await
}

pub(super) async fn audit_rows(db: &DatabaseConnection, ws: Uuid, name: &str) -> Vec<String> {
    row_text(
        db,
        "SELECT row_to_json(a)::text AS t FROM airway_load_audit a \
         WHERE workspace_id = $1 AND pipeline_name = $2 ORDER BY started_at, load_id",
        vec![ws.into(), name.into()],
    )
    .await
}

pub(super) fn preview_name(name: &str) -> String {
    agentic_pipeline::airway_preview::scoped_pipeline_name(KEY, name)
}
