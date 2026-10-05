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

// "Local mode serves no organization routes and no GitHub namespace routes"
// used to be asserted here, and could not fail. The org tree and the GitHub
// surface are mounted by `oxy-server` through the api seam, so a router built
// with `SurfaceSeams::empty()` has neither in ANY mode — and the paths asked
// for (`/organizations…`, `/github/namespaces…`) are ones no crate registers at
// all; the real ones are `/orgs…` and `/orgs/{org_id}/github/namespaces…`.
// Both directions — cloud mounts them, local drops them — are asserted on the
// real paths in `oxy-server`'s `served_router_tests`, which builds this router
// from the seams boot hands it.

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
