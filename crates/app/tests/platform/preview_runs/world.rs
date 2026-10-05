//! The held-runs fixture: the previews fixture's workspace and branch, plus a
//! procedure at [`PROCEDURE`] in both revisions, a staging config naming a
//! ClickHouse no process can reach and a managed Airhouse this server has no
//! credentials for — so any write that is not held fails the run — and a
//! driver that plays the global-run latency worker.

use std::sync::Arc;
use std::time::{Duration, Instant};

use agentic_pipeline::platform::{PlatformContext, RunPlatformResolver};
use agentic_runtime::state::RuntimeState;
use axum::http::StatusCode;
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy_app::agentic_wiring::OxyProjectContext;
use oxy_app::server::previews::runtime::PreviewRunResolver;
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::preview_routes::fixture::{BRANCH, Fx, exec, send_json, setup};

pub(crate) const PROCEDURE: &str = "workflows/je.procedure.yml";

/// ClickHouse on a port nothing listens on, and a managed Airhouse.
pub(crate) fn databases() -> Value {
    json!([
        { "name": "clickhouse", "type": "clickhouse", "host": "http://127.0.0.1:1",
          "user": "default", "database": "default" },
        { "name": "airhouse", "type": "airhouse_managed" }
    ])
}

/// The previews fixture with `tasks` as the procedure in both revisions.
pub(crate) async fn world(tasks: Value) -> Fx {
    let fx = setup().await;
    let main = main_revision(&fx).await;
    for rev in [main, fx.staging] {
        exec(
            &fx.db,
            "UPDATE workspace_compiled_configs SET databases = $1 WHERE revision_id = $2",
            vec![databases().into(), rev.into()],
        )
        .await;
    }
    for rev in [main, fx.staging] {
        exec(
            &fx.db,
            "INSERT INTO automation_definitions (revision_id, file_path, name, extension, definition) \
             VALUES ($1, $2, 'je', 'procedure', $3)",
            vec![rev.into(), PROCEDURE.into(), json!({ "name": "je", "tasks": tasks.clone() }).into()],
        )
        .await;
    }
    fx
}

pub(crate) async fn main_revision(fx: &Fx) -> Uuid {
    entity_workspace(&fx.db, fx.ws)
        .await
        .current_revision_id
        .expect("promoted")
}

async fn entity_workspace(db: &DatabaseConnection, ws: Uuid) -> entity::workspaces::Model {
    use sea_orm::EntityTrait;
    entity::workspaces::Entity::find_by_id(ws)
        .one(db)
        .await
        .unwrap()
        .unwrap()
}

pub(crate) fn enable_runs(on: bool) {
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        if on {
            std::env::set_var("OXY_PREVIEW_RUNS", "1");
        } else {
            std::env::remove_var("OXY_PREVIEW_RUNS");
        }
    }
}

/// `POST /previews/runs` as staff.
pub(crate) async fn submit(fx: &Fx, branch: &str, target_ref: &str) -> (StatusCode, Value) {
    let body = json!({ "branch": branch, "kind": "procedure", "ref": target_ref,
                       "variables": { "date": "2026-09-27" } });
    send_json(
        &fx.staff,
        "POST",
        format!("/{}/previews/runs", fx.ws),
        Some(body),
    )
    .await
}

/// The production context of the workspace, at its promoted revision — the
/// base the driver hands the resolver.
pub(crate) async fn base_ctx(fx: &Fx, root: &std::path::Path) -> Arc<OxyProjectContext> {
    std::fs::write(root.join("config.yml"), "databases: []\nmodels: []\n").unwrap();
    let manager = WorkspaceBuilder::new(fx.ws)
        .with_working_copy(
            root,
            Some(main_revision(fx).await),
            oxy::config::OnMissing::Empty,
        )
        .await
        .expect("base config")
        .build()
        .await
        .expect("base manager");
    Arc::new(OxyProjectContext::new(manager))
}

/// Drive the workspace's pending Global runs the way the latency worker does,
/// with `resolver` (the production one, or one whose preview Airhouse is a
/// stand-in), until `run_id` is terminal. Its status.
pub(crate) async fn drive_with(
    fx: &Fx,
    run_id: &str,
    resolver: Arc<dyn RunPlatformResolver>,
) -> String {
    let root = tempfile::tempdir().unwrap();
    let base: Arc<dyn PlatformContext> = base_ctx(fx, root.path()).await;
    let state = Arc::new(RuntimeState::new());
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        agentic_pipeline::recovery::recover_pending_global_runs(
            fx.db.clone(),
            state.clone(),
            base.clone(),
            resolver.clone(),
            None,
            None,
            None,
            None,
            Arc::new(agentic_runtime::router::NoopTaskRouter),
            Some(fx.ws),
            None,
            agentic_pipeline::recovery::DrivePolicy::ALL,
        )
        .await;
        let run = agentic_runtime::crud::get_run(&fx.db, run_id)
            .await
            .unwrap()
            .expect("run row");
        let status = run.task_status.unwrap_or_default();
        if matches!(
            status.as_str(),
            "done" | "failed" | "cancelled" | "timed_out"
        ) {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "run {run_id} still `{status}`: {:?}",
            run.error_message
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Submit, drive to the end, sweep, and read the run back.
pub(crate) async fn run_to_the_end(fx: &Fx) -> Value {
    run_to_the_end_with(fx, PreviewRunResolver::shared(&fx.db)).await
}

/// [`run_to_the_end`], driven with `resolver`.
pub(crate) async fn run_to_the_end_with(fx: &Fx, resolver: Arc<dyn RunPlatformResolver>) -> Value {
    enable_runs(true);
    let (status, body) = submit(fx, BRANCH, PROCEDURE).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let run_id = body["run_id"].as_str().unwrap().to_string();
    drive_with(fx, &run_id, resolver).await;
    oxy_app::server::previews::runs::sweep(&fx.db).await;
    let (status, detail) = send_json(
        &fx.staff,
        "GET",
        format!("/{}/previews/runs/{run_id}", fx.ws),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail
}
