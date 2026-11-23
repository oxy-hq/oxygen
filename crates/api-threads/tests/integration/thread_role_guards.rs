//! Coverage for the WorkspaceEditor guard on destructive thread endpoints.
//!
//! `delete_all_threads` and `bulk_delete_threads` are gated by the
//! `WorkspaceEditor` extractor as their first parameter. Because axum runs
//! extractors in declaration order and short-circuits on the first
//! rejection, we can verify the 403-for-viewer contract without ever
//! reaching the DB / WorkspaceManager extractors that follow.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{delete, post};
use entity::workspace_members::WorkspaceRole;
use oxy_api_threads::thread::{bulk_delete_threads, delete_all_threads};
use oxy_server_authz::workspace_role::EffectiveWorkspaceRole;
use tower::ServiceExt;

/// Inserts an `EffectiveWorkspaceRole` extension so downstream extractors
/// (specifically `WorkspaceEditor`) see a resolved role.
fn inject_role(
    role: WorkspaceRole,
) -> impl Clone + Fn(Request<Body>, Next) -> futures::future::BoxFuture<'static, Response> {
    move |mut req: Request<Body>, next: Next| {
        let role = role.clone();
        Box::pin(async move {
            req.extensions_mut().insert(EffectiveWorkspaceRole(role));
            next.run(req).await
        })
    }
}

// `delete_all_threads` asks for a working copy (a best-effort chart sweep), so
// it resolves from `IdeState` and the test router must carry one.
fn build_router(role: WorkspaceRole) -> Router {
    Router::new()
        .route("/threads", delete(delete_all_threads))
        .route("/threads/bulk-delete", post(bulk_delete_threads))
        .layer(middleware::from_fn(inject_role(role)))
        .with_state(oxy_app_core::pods::IdeState(
            oxy_app_core::app_state::bare_app_state(),
        ))
}

async fn status_for(router: Router, method: &str, path: &str) -> StatusCode {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from("{\"thread_ids\":[]}"))
        .unwrap();
    router.oneshot(req).await.expect("oneshot").status()
}

#[tokio::test]
async fn delete_all_threads_rejects_viewer_with_403() {
    let router = build_router(WorkspaceRole::Viewer);
    assert_eq!(
        status_for(router, "DELETE", "/threads").await,
        StatusCode::FORBIDDEN,
        "WorkspaceEditor must reject Viewer on delete_all_threads"
    );
}

#[tokio::test]
async fn bulk_delete_threads_rejects_viewer_with_403() {
    let router = build_router(WorkspaceRole::Viewer);
    assert_eq!(
        status_for(router, "POST", "/threads/bulk-delete").await,
        StatusCode::FORBIDDEN,
        "WorkspaceEditor must reject Viewer on bulk_delete_threads"
    );
}

#[tokio::test]
async fn delete_all_threads_does_not_403_for_member() {
    // Member is allowed by WorkspaceEditor. The handler will fail at a
    // downstream extractor (no WorkspaceManager wired up), but the failure
    // must not be 403 — that would mean WorkspaceEditor wrongly rejected.
    let router = build_router(WorkspaceRole::Member);
    let status = status_for(router, "DELETE", "/threads").await;
    assert_ne!(
        status,
        StatusCode::FORBIDDEN,
        "Member must pass WorkspaceEditor on delete_all_threads, got {status}"
    );
}

#[tokio::test]
async fn bulk_delete_threads_does_not_403_for_member() {
    let router = build_router(WorkspaceRole::Member);
    let status = status_for(router, "POST", "/threads/bulk-delete").await;
    assert_ne!(
        status,
        StatusCode::FORBIDDEN,
        "Member must pass WorkspaceEditor on bulk_delete_threads, got {status}"
    );
}
