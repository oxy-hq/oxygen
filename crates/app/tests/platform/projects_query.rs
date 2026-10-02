//! Integration tests for POST /api/projects/{project_id}/query.
//!
//! Skips automatically when `OXY_DATABASE_URL` is unset — `api_router`
//! now requires DB connectivity at construction time. To run locally:
//!
//!   OXY_DATABASE_URL=postgres://... cargo nextest run -p oxy-app \
//!     --test platform -E 'test(projects_query)'

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;
use tower::ServiceExt;

use oxy_app::server::router::api_router;
use oxy_app_core::serve_mode::ServeMode;

fn db_unavailable() -> bool {
    std::env::var("OXY_DATABASE_URL").is_err()
}

// Force auth-configured so missing-cookie requests reliably 401
// instead of falling through to the zero-config guest-user path
// (which would then 404 on the missing project — leaking project
// existence). Each test calls this to keep the auth state explicit.
fn force_auth_configured() {
    oxy_auth::built_in::set_auth_configured(true);
}

#[tokio::test]
async fn missing_cookie_returns_401() {
    if db_unavailable() {
        eprintln!("Skipping: OXY_DATABASE_URL not set");
        return;
    }
    force_auth_configured();
    // `api_router` returns (app_router, external_router, preagg_cache); these
    // tests exercise the main app surface (`/projects/...`), so the external
    // router and the cache handle are unused.
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
    .expect("router built");

    let body = serde_json::to_vec(&json!({ "sql": "SELECT 1" })).unwrap();
    let req = Request::builder()
        .method("POST")
        .uri("/projects/00000000-0000-0000-0000-000000000000/query")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn empty_body_with_no_cookie_returns_401() {
    if db_unavailable() {
        eprintln!("Skipping: OXY_DATABASE_URL not set");
        return;
    }
    force_auth_configured();
    // `api_router` returns (app_router, external_router, preagg_cache); these
    // tests exercise the main app surface (`/projects/...`), so the external
    // router and the cache handle are unused.
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
    .expect("router built");

    let req = Request::builder()
        .method("POST")
        .uri("/projects/00000000-0000-0000-0000-000000000000/query")
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap();

    // The handler takes the body as raw `Bytes` so auth runs BEFORE
    // body parsing. A missing cookie returns 401 even when the body
    // is malformed — see the comment in `run_query` for the security
    // rationale (no extractor-error leak of route existence / body
    // shape to unauthenticated callers).
    let resp = router.oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
