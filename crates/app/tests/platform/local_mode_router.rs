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

// Was `local_router_has_public_liveness_route` (`GET /live` = 200). The probes
// are `oxy-server`'s since 2026-11-16 (`core_routes`), so a router built with
// `SurfaceSeams::empty()` no longer has `/live`;
// `served_router_tests::both_modes_mount_the_health_probes` holds it on the
// served router. What this asks of the local router is the same: its public
// tree is mounted.
#[tokio::test]
async fn local_router_has_its_public_tree() {
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
    // The QuickBooks OAuth callback is public-tree and GET only: TRACE gets
    // 405 where it is mounted and 404 where it is not, without reaching the
    // handler. (The probe was the Stripe webhook until it left for
    // `oxy-api-billing` in the admin split's PR 9; `served_router_tests` holds
    // it on the served router, in both modes.)
    for (uri, want) in [
        ("/quickbooks/oauth/callback", StatusCode::METHOD_NOT_ALLOWED),
        ("/webhooks/nothing-mounts-this", StatusCode::NOT_FOUND),
    ] {
        let req = Request::builder()
            .method(axum::http::Method::TRACE)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.expect("oneshot");
        assert_eq!(resp.status(), want, "{uri}");
    }
}

// Cloud-mode 404 coverage for /setup/* lives in router/workspace.rs
// (setup_routes_absent_when_include_local_setup_false). It drives
// build_workspace_routes directly without workspace_middleware, so no DB
// setup is required. Trying to assert the same behavior through the full
// api_router here would trip over workspace_middleware hitting an
// unavailable DB, returning 500 instead of 404.
