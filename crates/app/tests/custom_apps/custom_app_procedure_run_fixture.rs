//! Shared fixture for `custom_app_procedure_run_queue` — no tests of its own.
//! An earlier attempt's traces (a begun run, a dead claim) are
//! `custom_app_procedure_run_attempts`.
//!
//! **Replicas.** Each [`replica`] is a router over the three production
//! procedure-run handlers with an `AppState` and `AgenticState` of its own, so
//! two of them share nothing but the database — which is what two serve pods
//! share. `production_mounts_the_three_routes_here` pins the paths to
//! `public.rs`.
//!
//! **Driver.** Nothing in a replica drives a run. [`spawn_driver`] drains the
//! queue through `recover_pending_global_runs` (what
//! `router::recovery::drive_pending` calls) with the production
//! `ProcedureRunExecutor`, in a `CustomTaskRegistry` and loop of its own:
//! production's registry is private, and `router::recovery`'s own tests cover
//! its registration, the per-role drive policy and a drive through
//! `drive_pending`. The driver's base platform ([`driver_platform`]) is a
//! different, subject-less workspace on purpose — it is the context a worker
//! tick builds, and the one a procedure run must *not* execute with.
//!
//! **Identity.** `BuiltInAuthenticator` falls back to the guest when no auth
//! method is configured; `seeded_tenant` makes that guest the org's Owner.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use agentic_pipeline::platform::PlatformContext;
use agentic_pipeline::recovery::recover_pending_global_runs;
use agentic_runtime::router::noop_router;
use agentic_runtime::state::RuntimeState;
use agentic_runtime::worker::CustomTaskRegistry;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::{get, post};
use entity::customer_app_procedure_runs as proc_run;
use entity::workspaces;
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy::config::OnMissing;
use oxy_app::agentic_wiring::OxyProjectContext;
use oxy_app::agentic_wiring::thread_owner::OxyThreadOwnerLookup;
use oxy_app::server::api::projects::automation_run::executor::{
    ProcedureRunExecutor, STARTED_EVENT,
};
use oxy_app::server::api::projects::automation_run::task::PROCEDURE_RUN_KIND;
use oxy_app::server::api::projects::automation_run::{
    cancel_automation_run, poll_automation_run, start_automation_run,
};
use oxy_app::server::router::AppState;
use oxy_app_core::serve_mode::ServeMode;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection, EntityTrait};
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use uuid::Uuid;

use crate::custom_app_functions_fixture::{Tenant, seeded_tenant};

pub(crate) const START_ROUTE: &str = "/projects/{project_id}/procedures/{procedure_id}/runs";
pub(crate) const POLL_ROUTE: &str = "/projects/{project_id}/procedures/runs/{run_id}";
pub(crate) const CANCEL_ROUTE: &str = "/projects/{project_id}/procedures/runs/{run_id}/cancel";

/// One driver pass and an inline formatter step take well under a second; the
/// rest is slack for a cold CI box.
pub(crate) const RUN_DEADLINE: Duration = Duration::from_secs(60);

/// No warehouse, no LLM: one inline step whose output proves the request's
/// `params` reached the run through the queue.
const GREET_AUTOMATION: &str = r#"
name: greet
tasks:
  - name: greeting
    type: formatter
    template: "hello {{ params.store }}"
"#;

pub(crate) struct Fixture {
    pub(crate) t: Tenant,
    pub(crate) workspace_id: Uuid,
    pub(crate) _workspace_dir: tempfile::TempDir,
}

/// A workspace of the Local org with one automation, `greet`, on disk.
pub(crate) async fn fixture() -> Fixture {
    let t = seeded_tenant().await;
    let dir = tempfile::tempdir().expect("workspace dir");
    std::fs::write(dir.path().join("config.yml"), "databases: []\nmodels: []\n")
        .expect("write config.yml");
    std::fs::create_dir_all(dir.path().join("procedures")).expect("mkdir procedures");
    std::fs::write(
        dir.path().join("procedures/greet.automation.yml"),
        GREET_AUTOMATION,
    )
    .expect("write automation");

    let workspace_id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(workspace_id),
        name: ActiveValue::Set(format!("procedures-{workspace_id}")),
        org_id: ActiveValue::Set(Some(t.org_id)),
        path: ActiveValue::Set(Some(dir.path().to_string_lossy().to_string())),
        ..Default::default()
    }
    .insert(&t.db)
    .await
    .expect("seed workspace");
    Fixture {
        t,
        workspace_id,
        _workspace_dir: dir,
    }
}

/// One serve replica: the three production handlers over state of its own.
pub(crate) fn replica(db: &DatabaseConnection) -> Router {
    let agentic = agentic_http::AgenticState::new(
        CancellationToken::new(),
        db.clone(),
        Arc::new(OxyThreadOwnerLookup::new(db.clone())),
    );
    let state = AppState {
        enterprise: false,
        internal: false,
        mode: ServeMode::Cloud,
        observability: None,
        startup_cwd: PathBuf::new(),
        preagg_cache: None,
        preagg_renewal_threshold_secs: None,
        agentic_state: Some(Arc::new(agentic)),
        semantic_layer_cache: oxy_app_core::workspace_cache::new_semantic_layer_cache(),
        semantic_engine_cache: oxy_app_core::workspace_cache::new_semantic_engine_cache(),
    };
    Router::new()
        .route(START_ROUTE, post(start_automation_run))
        .route(POLL_ROUTE, get(poll_automation_run))
        .route(CANCEL_ROUTE, post(cancel_automation_run))
        .with_state(state)
}

pub(crate) async fn send(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.expect("oneshot");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("read body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// `POST …/procedures/greet/runs`, asserting the start contract: 202 and a
/// body that is `{ "run_id": "<uuid>" }` and nothing else.
pub(crate) async fn start(app: &Router, workspace_id: Uuid) -> String {
    let (status, body) = send(
        app,
        Request::post(format!("/projects/{workspace_id}/procedures/greet/runs"))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "v": 1, "params": { "store": "s-7" } }).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "start: {body}");
    let object = body.as_object().expect("start answers an object");
    assert_eq!(
        object.keys().collect::<Vec<_>>(),
        vec!["run_id"],
        "start: {body}"
    );
    let run_id = object["run_id"].as_str().expect("run_id is a string");
    Uuid::parse_str(run_id).expect("run_id is a uuid");
    run_id.to_string()
}

pub(crate) async fn poll(app: &Router, workspace_id: Uuid, run_id: &str) -> Value {
    let uri = format!("/projects/{workspace_id}/procedures/runs/{run_id}");
    let (status, body) = send(app, Request::get(&uri).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK, "GET {uri}: {body}");
    body
}

/// Poll until the run leaves `running`; panics at the deadline.
pub(crate) async fn wait_for_run(app: &Router, workspace_id: Uuid, run_id: &str) -> Value {
    let deadline = Instant::now() + RUN_DEADLINE;
    let mut last = Value::Null;
    while Instant::now() < deadline {
        let body = poll(app, workspace_id, run_id).await;
        if body["status"] != "running" {
            return body;
        }
        last = body;
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("procedure run {run_id} did not finish within {RUN_DEADLINE:?}; last poll: {last}");
}

/// The context a driver tick hands `recover_pending_global_runs`: a workspace
/// that is not the run's, with no subject — what `build_cloud_project_ctx`
/// builds on a worker.
pub(crate) async fn driver_platform() -> (Arc<OxyProjectContext>, tempfile::TempDir) {
    let root = tempfile::tempdir().expect("platform dir");
    std::fs::write(
        root.path().join("config.yml"),
        "databases: []\nmodels: []\n",
    )
    .expect("write config.yml");
    let manager = WorkspaceBuilder::new(Uuid::new_v4())
        .with_working_copy(root.path(), None, OnMissing::Fail)
        .await
        .expect("config.yml loads")
        .build()
        .await
        .expect("workspace manager");
    (Arc::new(OxyProjectContext::new(manager)), root)
}

fn registry(db: &DatabaseConnection) -> Arc<CustomTaskRegistry> {
    let mut registry = CustomTaskRegistry::new();
    registry.register(
        PROCEDURE_RUN_KIND,
        Arc::new(ProcedureRunExecutor { db: db.clone() }),
    );
    Arc::new(registry)
}

/// One pass of the latency worker over `workspace_id`'s Global runs. Returns
/// how many it took in hand.
pub(crate) async fn drive_once(
    db: &DatabaseConnection,
    state: &Arc<RuntimeState>,
    platform: &Arc<OxyProjectContext>,
    workspace_id: Uuid,
) -> usize {
    let platform: Arc<dyn PlatformContext> = platform.clone();
    recover_pending_global_runs(
        db.clone(),
        state.clone(),
        platform,
        Arc::new(agentic_pipeline::platform::IdentityResolver),
        None,
        None,
        None,
        None,
        noop_router(),
        Some(workspace_id),
        Some(registry(db)),
        // A node that leaves nothing for another: a worker's policy as far as
        // this kind goes, and `OXY_ROLE=all`'s outright.
        agentic_pipeline::recovery::DrivePolicy::ALL,
    )
    .await
}

/// A driver process: its own `RuntimeState`, polling as the latency worker does.
pub(crate) fn spawn_driver(
    db: DatabaseConnection,
    platform: Arc<OxyProjectContext>,
    workspace_id: Uuid,
) -> JoinHandle<()> {
    let state = Arc::new(RuntimeState::new());
    tokio::spawn(async move {
        loop {
            drive_once(&db, &state, &platform, workspace_id).await;
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
}

pub(crate) async fn queue_status(db: &DatabaseConnection, run_id: &str) -> Option<String> {
    agentic_runtime::crud::get_queue_entry(db, run_id)
        .await
        .expect("queue lookup")
        .map(|entry| entry.queue_status)
}

/// The run's own row — what the poll endpoint reads, plus the columns it
/// deliberately never serves (`execution_started_at`, `execution_heartbeat_at`).
pub(crate) async fn run_row(db: &DatabaseConnection, run_id: &str) -> proc_run::Model {
    proc_run::Entity::find_by_id(Uuid::parse_str(run_id).expect("run id"))
        .one(db)
        .await
        .expect("run row lookup")
        .expect("the run's row")
}

pub(crate) async fn started_events(db: &DatabaseConnection, run_id: &str) -> Vec<Value> {
    agentic_runtime::crud::get_all_events(db, run_id)
        .await
        .expect("events")
        .into_iter()
        .filter(|event| event.event_type == STARTED_EVENT)
        .map(|event| event.payload)
        .collect()
}

/// Wait until the driver has settled the run's own row, whatever it settled as.
pub(crate) async fn wait_for_driver(db: &DatabaseConnection, run_id: &str) -> String {
    let deadline = Instant::now() + RUN_DEADLINE;
    while Instant::now() < deadline {
        let run = agentic_runtime::crud::get_run(db, run_id)
            .await
            .expect("run lookup")
            .expect("run row");
        if let Some(status @ ("done" | "failed" | "cancelled")) = run.task_status.as_deref() {
            return status.to_string();
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("the driver never settled run {run_id}");
}
