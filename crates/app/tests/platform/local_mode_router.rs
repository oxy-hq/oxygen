//! Integration coverage for `api_router(ServeMode::Local)`.
//!
//! Drives the router via `tower::ServiceExt::oneshot` — no HTTP listener.
//! The requests exercised below don't reach the DB at runtime, but
//! `api_router()` itself wires database-backed middleware during build, so
//! the tests skip when `OXY_DATABASE_URL` is unset to avoid flagging
//! environment-config failures as code regressions.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxy_app::server::router::api_router;
use oxy_app_core::serve_mode::ServeMode;
use tower::ServiceExt;

fn db_unavailable() -> bool {
    std::env::var("OXY_DATABASE_URL").is_err()
}

#[tokio::test]
async fn local_router_returns_404_for_organization_routes() {
    if db_unavailable() {
        return;
    }
    let (router, _external, _preagg) = api_router(
        ServeMode::Local,
        false,
        None,
        std::path::PathBuf::new(),
        tokio_util::sync::CancellationToken::new(),
        false,
        oxy_app::server::router::SurfaceSeams::empty(),
    )
    .await
    .expect("build router");
    for path in [
        "/organizations",
        "/organizations/acme",
        "/organizations/acme/members",
        "/organizations/acme/invitations",
    ] {
        let req = Request::builder().uri(path).body(Body::empty()).unwrap();
        let resp = router
            .clone()
            .oneshot(req)
            .await
            .expect("oneshot succeeded");
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "{} should be absent in local mode, got {}",
            path,
            resp.status()
        );
    }
}

#[tokio::test]
async fn local_router_returns_404_for_github_namespace_routes() {
    if db_unavailable() {
        return;
    }
    let (router, _external, _preagg) = api_router(
        ServeMode::Local,
        false,
        None,
        std::path::PathBuf::new(),
        tokio_util::sync::CancellationToken::new(),
        false,
        oxy_app::server::router::SurfaceSeams::empty(),
    )
    .await
    .expect("build router");
    for path in ["/github/namespaces", "/github/namespaces/pat"] {
        let req = Request::builder().uri(path).body(Body::empty()).unwrap();
        let resp = router
            .clone()
            .oneshot(req)
            .await
            .expect("oneshot succeeded");
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "{} should be absent in local mode",
            path
        );
    }
}

#[tokio::test]
async fn local_router_has_public_liveness_route() {
    // Use /live instead of /health: /health returns 503 when DB is unreachable
    // (which is the case in unit tests); /live is the unconditional liveness
    // endpoint and always returns 200.
    if db_unavailable() {
        return;
    }
    let (router, _external, _preagg) = api_router(
        ServeMode::Local,
        false,
        None,
        std::path::PathBuf::new(),
        tokio_util::sync::CancellationToken::new(),
        false,
        oxy_app::server::router::SurfaceSeams::empty(),
    )
    .await
    .expect("build router");
    let req = Request::builder().uri("/live").body(Body::empty()).unwrap();
    let resp = router.oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);
}

// Cloud-mode 404 coverage for /setup/* lives in router/workspace.rs
// (setup_routes_absent_when_include_local_setup_false). It drives
// build_workspace_routes directly without workspace_middleware, so no DB
// setup is required. Trying to assert the same behavior through the full
// api_router here would trip over workspace_middleware hitting an
// unavailable DB, returning 500 instead of 404.
