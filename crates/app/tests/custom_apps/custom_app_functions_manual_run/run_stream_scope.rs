//! A custom app's run stream and cancel act on a run of the project in the
//! path, and on no other:
//! `GET /projects/{project_id}/agents/runs/{run_id}/events` and
//! `POST /projects/{project_id}/agents/asks/{run_id}/cancel`.
//!
//! The gates admit the caller to the project; the run id is still the
//! caller's to type. A run of another workspace, and a check run staff queued
//! in this one outside production, answer the same not-found as an id that
//! names nothing — a member holding an id can neither confirm the run nor
//! cancel it. The project's own run streams and cancels as before.
//!
//! The handlers are mounted as `router/public.rs` mounts them, behind the
//! real gate chain; the caller is the guest, an Owner of the project's org.

use std::sync::Arc;

use agentic_http::AgenticState;
use agentic_pipeline::platform::ThreadOwnerLookup;
use agentic_pipeline::scheduler::enqueue_app_function_job_in;
use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::{get, post};
use oxy_app::server::api::projects::{agent_ask, agent_run_stream};
use oxy_app::server::router::bare_app_state;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{Tenant, seeded_tenant};

const STREAM_ROUTE: &str = "/projects/{project_id}/agents/runs/{run_id}/events";
const CANCEL_ROUTE: &str = "/projects/{project_id}/agents/asks/{run_id}/cancel";

/// Every run here is threadless.
struct NoThreads;

#[async_trait]
impl ThreadOwnerLookup for NoThreads {
    async fn thread_owner(&self, _thread: Uuid) -> Result<Option<Option<Uuid>>, String> {
        Ok(None)
    }
}

fn project_routes(t: &Tenant) -> Router {
    let mut state = bare_app_state();
    state.agentic_state = Some(Arc::new(AgenticState::new(
        CancellationToken::new(),
        t.db.clone(),
        Arc::new(NoThreads),
    )));
    Router::new()
        .route(STREAM_ROUTE, get(agent_run_stream::stream_agent_run))
        .route(CANCEL_ROUTE, post(agent_ask::cancel_ask))
        .with_state(state)
}

/// The status of `method path`. An SSE body never ends, so none is read.
async fn status_of(t: &Tenant, method: &str, path: &str) -> StatusCode {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .expect("request");
    project_routes(t)
        .oneshot(request)
        .await
        .expect("oneshot")
        .status()
}

async fn stream(t: &Tenant, run_id: &str) -> StatusCode {
    let project = demo_workspace_id();
    let path = format!("/projects/{project}/agents/runs/{run_id}/events");
    status_of(t, "GET", &path).await
}

async fn cancel(t: &Tenant, run_id: &str) -> StatusCode {
    let project = demo_workspace_id();
    let path = format!("/projects/{project}/agents/asks/{run_id}/cancel");
    status_of(t, "POST", &path).await
}

/// An analytics run in `workspace`, as a custom app's ask seeds one.
async fn analytics_run(t: &Tenant, workspace: Uuid) -> String {
    let id = Uuid::new_v4().to_string();
    agentic_runtime::crud::insert_run(
        &t.db,
        &id,
        "q",
        None,
        "analytics",
        Some(json!({})),
        workspace,
    )
    .await
    .expect("insert_run");
    id
}

async fn task_status(t: &Tenant, run_id: &str) -> Option<String> {
    agentic_runtime::crud::get_run(&t.db, run_id)
        .await
        .expect("get_run")
        .expect("the run exists")
        .task_status
}

#[tokio::test]
async fn a_run_is_streamed_and_cancelled_only_through_its_own_project() {
    let t = seeded_tenant().await;
    let project = demo_workspace_id();
    let mine = analytics_run(&t, project).await;
    // Another workspace's run, and a check run staff queued here in staging.
    let theirs = analytics_run(&t, Uuid::new_v4()).await;
    let app = Uuid::new_v4().to_string();
    let check = enqueue_app_function_job_in(
        &t.db,
        &app,
        "smoke",
        project,
        None,
        "manual",
        None,
        None,
        Some("staging"),
    )
    .await
    .expect("queue a staging check");
    let unknown = Uuid::new_v4().to_string();

    for (what, run_id) in [
        ("another workspace's run", &theirs),
        ("a check run outside production", &check),
        ("an id that names nothing", &unknown),
    ] {
        assert_eq!(
            stream(&t, run_id).await,
            StatusCode::NOT_FOUND,
            "stream {what}"
        );
        assert_eq!(
            cancel(&t, run_id).await,
            StatusCode::NOT_FOUND,
            "cancel {what}"
        );
    }
    // Refused, and untouched: neither run was cancelled on the way.
    let before = task_status(&t, &mine).await;
    assert_eq!(task_status(&t, &theirs).await, before, "theirs was changed");
    assert_eq!(
        task_status(&t, &check).await,
        before,
        "the check was changed"
    );

    // The project's own run streams, and its member cancels it.
    assert_eq!(stream(&t, &mine).await, StatusCode::OK);
    assert_eq!(cancel(&t, &mine).await, StatusCode::NO_CONTENT);
}
