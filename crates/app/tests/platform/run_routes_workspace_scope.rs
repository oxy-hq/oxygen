//! Regression: a run id names a run in ONE workspace.
//!
//! Every `/runs/{id}/…` route under `/analytics`, `/agentic-workflows` and
//! `/agentic-airway` is mounted below `/{workspace_id}`, whose middleware
//! checks the caller belongs to THAT workspace — and then acted on whatever run
//! the id named. So a member of workspace A could stream workspace B's run
//! events (LLM output, SQL results), cancel B's run (rewriting it to "failed,
//! cancelled by user"), or read its snapshot, given only the id. And
//! `GET /agentic-workflows/runs?workflow_ref=` handed out those ids: it filtered
//! on the workspace-relative path alone, which every workspace seeded from the
//! same template shares.
//!
//! Drives the real agentic-http routers with the extensions the workspace
//! middleware installs — a `PlatformContext` for workspace A and a signed-in
//! user — against the runtime tables, and asks about B's runs. Each must
//! answer 404, exactly like a run that does not exist, and change nothing.
//!
//! Database-backed through [`crate::common::fresh_db`] with every migrator `oxy serve` runs.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agentic_http::AgenticState;
use agentic_pipeline::platform::{PlatformContext, ProjectContext, ThreadOwnerLookup};
use agentic_runtime::lifecycle::crud::runs::insert_run;
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::{Extension, Router};
use entity::users::UserStatus;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::DatabaseConnection;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use uuid::Uuid;

const WORKFLOW_REF: &str = "automations/daily.automation.yml";

/// The workspace the request is scoped to — what the workspace middleware
/// hands every agentic route. Nothing else on it is reachable from these routes.
struct Platform {
    workspace_id: Uuid,
}

#[async_trait]
impl ProjectContext for Platform {
    async fn resolve_connector(&self, _db: &str) -> Option<agentic_connector::ConnectorConfig> {
        None
    }
    async fn resolve_model(
        &self,
        _model_ref: Option<&str>,
        _explicit: bool,
    ) -> Option<agentic_analytics::config::ResolvedModelInfo> {
        None
    }
    async fn resolve_secret(&self, _var: &str) -> Option<String> {
        None
    }
    fn workspace_id(&self) -> Uuid {
        self.workspace_id
    }
}

#[async_trait]
impl agentic_automation::WorkspaceContext for Platform {
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
}

/// Every run here is threadless, like scheduled and manual runs.
struct NoThreads;

#[async_trait]
impl ThreadOwnerLookup for NoThreads {
    async fn thread_owner(&self, _thread: Uuid) -> Result<Option<Option<Uuid>>, String> {
        Ok(None)
    }
}

/// The three agentic routers, as a member of `workspace_id` reaches them.
fn routes_as_member_of(db: &DatabaseConnection, workspace_id: Uuid) -> Router {
    let state = Arc::new(AgenticState::new(
        CancellationToken::new(),
        db.clone(),
        Arc::new(NoThreads),
    ));
    let platform: Arc<dyn PlatformContext> = Arc::new(Platform { workspace_id });
    let user = AuthenticatedUser {
        id: Uuid::new_v4(),
        email: Some("member@acme.test".into()),
        name: "Member".into(),
        picture: None,
        status: UserStatus::Active,
    };
    Router::new()
        .nest("/analytics", agentic_http::router(state.clone()))
        .nest(
            "/agentic-workflows",
            agentic_http::automation_router(state.clone()),
        )
        .nest("/agentic-airway", agentic_http::airway_router(state))
        .layer(Extension(platform))
        .layer(Extension(user))
}

async fn seed(db: &DatabaseConnection, ws: Uuid, id: &str, source_type: &str) {
    let metadata =
        json!({ "workflow_ref": WORKFLOW_REF, "pipeline_ref": "pipelines/p.airway.yml" });
    insert_run(db, id, "a run", None, source_type, Some(metadata), ws)
        .await
        .expect("insert run");
}

async fn call(router: &Router, method: &str, path: &str) -> (StatusCode, Vec<u8>) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap();
    let resp = router.clone().oneshot(req).await.expect("oneshot");
    let status = resp.status();
    // An SSE body never ends; only a refusal's body is read.
    let body = if status.is_success() {
        Vec::new()
    } else {
        axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .map(|b| b.to_vec())
            .unwrap_or_default()
    };
    (status, body)
}

async fn status_of(db: &DatabaseConnection, id: &str) -> Option<String> {
    agentic_runtime::crud::get_run(db, id)
        .await
        .expect("get run")
        .expect("run exists")
        .task_status
}

#[tokio::test]
async fn another_workspaces_run_is_not_found_on_every_run_route() {
    let (db, _url) = crate::common::fresh_db(crate::common::Schema::All).await;
    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    seed(&db, b, "analytics-b", "analytics").await;
    seed(&db, b, "workflow-b", "workflow").await;
    seed(&db, b, "airway-b", "airway").await;
    let as_a = routes_as_member_of(&db, a);

    for (method, path) in [
        ("GET", "/analytics/runs/analytics-b/events"),
        ("POST", "/analytics/runs/analytics-b/cancel"),
        ("PATCH", "/analytics/runs/analytics-b/thinking_mode"),
        ("GET", "/agentic-workflows/runs/workflow-b"),
        ("GET", "/agentic-workflows/runs/workflow-b/events"),
        ("POST", "/agentic-workflows/runs/workflow-b/cancel"),
        ("GET", "/agentic-airway/runs/airway-b/events"),
        ("POST", "/agentic-airway/runs/airway-b/cancel"),
    ] {
        let (status, body) = call(&as_a, method, path).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{method} {path} from another workspace: {}",
            String::from_utf8_lossy(&body)
        );
        assert_eq!(
            body, b"run not found",
            "{method} {path}: same answer as a missing run"
        );
    }

    // Nothing was written: the cancels used to fail B's runs outright.
    for id in ["analytics-b", "workflow-b", "airway-b"] {
        assert_eq!(status_of(&db, id).await.as_deref(), Some("running"), "{id}");
    }
}

#[tokio::test]
async fn a_workspaces_own_runs_still_answer() {
    let (db, _url) = crate::common::fresh_db(crate::common::Schema::All).await;
    let a = Uuid::new_v4();
    seed(&db, a, "analytics-a", "analytics").await;
    seed(&db, a, "airway-a", "airway").await;
    let as_a = routes_as_member_of(&db, a);

    let (status, _) = call(&as_a, "GET", "/analytics/runs/analytics-a/events").await;
    assert_eq!(status, StatusCode::OK, "own run's event stream opens");
    let (status, _) = call(&as_a, "POST", "/agentic-airway/runs/airway-a/cancel").await;
    assert_eq!(status, StatusCode::NO_CONTENT, "own run cancels");
    assert_eq!(status_of(&db, "airway-a").await.as_deref(), Some("failed"));
}

#[tokio::test]
async fn an_automations_run_list_is_its_own_workspaces() {
    let (db, _url) = crate::common::fresh_db(crate::common::Schema::All).await;
    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    // Same workspace-relative path in both — as every workspace seeded from one
    // template has.
    seed(&db, a, "workflow-a", "workflow").await;
    seed(&db, b, "workflow-b", "workflow").await;

    let path = format!("/agentic-workflows/runs?workflow_ref={WORKFLOW_REF}");
    let req = Request::builder().uri(&path).body(Body::empty()).unwrap();
    let resp = routes_as_member_of(&db, a)
        .oneshot(req)
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    let runs: Vec<serde_json::Value> = serde_json::from_slice(&body).expect("json list");
    let ids: Vec<&str> = runs.iter().filter_map(|r| r["run_id"].as_str()).collect();
    assert_eq!(ids, ["workflow-a"], "B's run id must not be listed to A");
}
