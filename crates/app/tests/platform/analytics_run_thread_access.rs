//! Regression: `POST /analytics/runs` takes its `thread_id` from the body, and
//! the pipeline loads that thread's earlier questions and answers into the new
//! run as conversation history, then appends the run to the thread. Nothing
//! checked whose thread it was: a member who knew another member's thread id
//! got that conversation into their own run and wrote into it, and a thread id
//! from another workspace worked the same way.
//!
//! Drives the real analytics router with the real `OxyThreadOwnerLookup` over
//! a fresh database. The platform answers every agent id with an empty agent
//! definition, so a run the route lets through gets far enough into the
//! pipeline to be inserted on its thread — the history read is the next step.
//! A refused run must leave nothing on the thread.
//!
//! Database-backed through [`crate::common::fresh_db`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agentic_builder::{
    BuilderDatabaseProvider, BuilderProjectValidator, BuilderSchemaProvider,
    BuilderSemanticCompiler, PreaggHandle, SemanticCompilationResult, ValidationReport,
};
use agentic_core::{result::QueryResult, tools::ToolError};
use agentic_http::AgenticState;
use agentic_pipeline::platform::{BuilderBridges, PlatformContext, ProjectContext};
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::{Extension, Router};
use entity::users::UserStatus;
use entity::workspaces::WorkspaceStatus;
use entity::{threads, users, workspaces};
use oxy_app::agentic_wiring::OxyThreadOwnerLookup;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use uuid::Uuid;

/// Workspace `workspace_id`, whose every agent is the empty definition.
struct Platform {
    workspace_id: Uuid,
}

#[async_trait]
impl ProjectContext for Platform {
    async fn resolve_connector(&self, _db: &str) -> Option<agentic_connector::ConnectorConfig> {
        None
    }
    async fn resolve_agent_yaml(&self, _agent_id: &str) -> Option<String> {
        Some("{}".into())
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

/// The builder ports `create_run` requires as an extension. An analytics run
/// never reaches them.
struct NoBuilder;

fn unused() -> ToolError {
    ToolError::Execution("not reached by an analytics run".into())
}

#[async_trait]
impl BuilderDatabaseProvider for NoBuilder {
    async fn list_databases(&self) -> Result<Vec<String>, ToolError> {
        Err(unused())
    }
    async fn get_connector(
        &self,
        _name: &str,
    ) -> Result<Arc<dyn agentic_connector::DatabaseConnector>, ToolError> {
        Err(unused())
    }
}

#[async_trait]
impl BuilderProjectValidator for NoBuilder {
    async fn validate_file(&self, _abs_path: &Path) -> Result<(), String> {
        Err("unused".into())
    }
    async fn validate_all(&self) -> Result<ValidationReport, ToolError> {
        Err(unused())
    }
}

impl BuilderSchemaProvider for NoBuilder {
    fn get_schema(&self, _object_name: &str) -> Option<serde_json::Value> {
        None
    }
    fn supported_types(&self) -> &[&str] {
        &[]
    }
}

#[async_trait]
impl BuilderSemanticCompiler for NoBuilder {
    async fn compile(
        &self,
        _params: &serde_json::Value,
    ) -> Result<SemanticCompilationResult, ToolError> {
        Err(unused())
    }
    async fn execute_preagg(
        &self,
        _preagg_sql: &str,
        _handle: &PreaggHandle,
        _sample_limit: u64,
    ) -> Result<QueryResult, ToolError> {
        Err(unused())
    }
}

fn bridges() -> BuilderBridges {
    BuilderBridges {
        db_provider: Arc::new(NoBuilder),
        project_validator: Arc::new(NoBuilder),
        schema_provider: Arc::new(NoBuilder),
        semantic_compiler: Arc::new(NoBuilder),
        secrets_provider: None,
    }
}

/// The analytics router as `user`, a member of `workspace_id`, reaches it.
fn analytics_as(db: &DatabaseConnection, workspace_id: Uuid, user: &AuthenticatedUser) -> Router {
    let state = Arc::new(AgenticState::new(
        CancellationToken::new(),
        db.clone(),
        Arc::new(OxyThreadOwnerLookup::new(db.clone())),
    ));
    let platform: Arc<dyn PlatformContext> = Arc::new(Platform { workspace_id });
    Router::new()
        .nest("/analytics", agentic_http::router(state))
        .layer(Extension(platform))
        .layer(Extension(user.clone()))
        .layer(Extension(bridges()))
}

async fn seed_workspace(db: &DatabaseConnection, tag: &str) -> Uuid {
    let id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(id),
        name: ActiveValue::Set(format!("ws-{tag}")),
        org_id: ActiveValue::Set(None),
        path: ActiveValue::Set(Some(format!("/nonexistent/{id}"))),
        status: ActiveValue::Set(WorkspaceStatus::Ready),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    id
}

async fn seed_user(db: &DatabaseConnection, email: &str) -> AuthenticatedUser {
    let user = users::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        email: ActiveValue::Set(Some(email.to_string())),
        name: ActiveValue::Set(email.to_string()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        magic_link_token: ActiveValue::Set(None),
        magic_link_token_expires_at: ActiveValue::Set(None),
        status: ActiveValue::Set(UserStatus::Active),
        created_at: ActiveValue::NotSet,
        last_login_at: ActiveValue::NotSet,
    }
    .insert(db)
    .await
    .expect("seed user");
    AuthenticatedUser {
        id: user.id,
        email: user.email,
        name: user.name,
        picture: user.picture,
        status: user.status,
        credential: None,
    }
}

async fn seed_thread(db: &DatabaseConnection, workspace: Uuid, owner: Option<Uuid>) -> Uuid {
    let id = Uuid::new_v4();
    threads::ActiveModel {
        id: ActiveValue::Set(id),
        user_id: ActiveValue::Set(owner),
        created_at: ActiveValue::NotSet,
        title: ActiveValue::Set("revenue".into()),
        input: ActiveValue::Set("what was revenue last quarter?".into()),
        output: ActiveValue::Set(String::new()),
        source_type: ActiveValue::Set("analytics".into()),
        source: ActiveValue::Set("agents/x.agentic.yml".into()),
        references: ActiveValue::Set("[]".into()),
        is_processing: ActiveValue::Set(false),
        project_id: ActiveValue::Set(workspace),
        sandbox_info: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed thread");
    id
}

/// `POST /analytics/runs` on `thread`: the status and, for a refusal, the body.
async fn start_run(router: &Router, thread: Uuid) -> (StatusCode, String) {
    let body = json!({
        "agent_id": "agents/x.agentic.yml",
        "question": "and the quarter before?",
        "thread_id": thread.to_string(),
    });
    let req = Request::builder()
        .method("POST")
        .uri("/analytics/runs")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = router.clone().oneshot(req).await.expect("oneshot");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap_or_default();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn runs_on(db: &DatabaseConnection, thread: Uuid) -> usize {
    agentic_runtime::crud::get_runs_by_thread(db, thread)
        .await
        .expect("runs by thread")
        .len()
}

#[tokio::test]
async fn a_run_on_another_members_thread_is_refused_and_writes_nothing() {
    let (db, _url) = crate::common::fresh_db(crate::common::Schema::All).await;
    let ws = seed_workspace(&db, "a").await;
    let (alice, bob) = (
        seed_user(&db, "alice@acme.test").await,
        seed_user(&db, "bob@acme.test").await,
    );
    let alices = seed_thread(&db, ws, Some(alice.id)).await;

    let (status, body) = start_run(&analytics_as(&db, ws, &bob), alices).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body, "access denied", "the thread-runs route's refusal");
    assert_eq!(
        runs_on(&db, alices).await,
        0,
        "nothing appended to her thread"
    );

    // Her own follow-up still reaches the pipeline and lands on the thread.
    let (status, body) = start_run(&analytics_as(&db, ws, &alice), alices).await;
    assert!(
        status != StatusCode::FORBIDDEN && status != StatusCode::NOT_FOUND,
        "the owner is let through: {status} {body}"
    );
    assert_eq!(runs_on(&db, alices).await, 1, "{status} {body}");
}

#[tokio::test]
async fn a_thread_in_another_workspace_or_none_is_not_found() {
    let (db, _url) = crate::common::fresh_db(crate::common::Schema::All).await;
    let (a, b) = (
        seed_workspace(&db, "a").await,
        seed_workspace(&db, "b").await,
    );
    let alice = seed_user(&db, "alice@acme.test").await;
    // Her own thread, and an unowned one — both in the other workspace.
    let hers_in_b = seed_thread(&db, b, Some(alice.id)).await;
    let nobodys_in_b = seed_thread(&db, b, None).await;
    let as_alice_in_a = analytics_as(&db, a, &alice);

    for thread in [hers_in_b, nobodys_in_b, Uuid::new_v4()] {
        let (status, body) = start_run(&as_alice_in_a, thread).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{thread}: {body}");
        assert_eq!(body, "thread not found");
        assert_eq!(runs_on(&db, thread).await, 0);
    }
}

/// A thread auto-provisioned for a run started without one has no owner; the
/// id handed back is how a client asks its follow-up, so it stays usable by
/// the workspace's members — as its runs are already readable by them.
#[tokio::test]
async fn an_unowned_thread_in_this_workspace_still_takes_a_follow_up() {
    let (db, _url) = crate::common::fresh_db(crate::common::Schema::All).await;
    let ws = seed_workspace(&db, "a").await;
    let bob = seed_user(&db, "bob@acme.test").await;
    let nobodys = seed_thread(&db, ws, None).await;

    let (status, body) = start_run(&analytics_as(&db, ws, &bob), nobodys).await;
    assert!(
        status != StatusCode::FORBIDDEN && status != StatusCode::NOT_FOUND,
        "{status} {body}"
    );
    assert_eq!(runs_on(&db, nobodys).await, 1, "{status} {body}");
}
