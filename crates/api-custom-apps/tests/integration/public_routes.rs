//! The routes `public_routes()` mounts are the ones `oxy-server` merges beside
//! the public tree, and each is reachable. No database: the Host-resolved health
//! read answers before it authenticates when the `Host` names no custom app, and
//! that is the one answer that needs no credential.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use oxy_api_custom_apps::public_routes;
use oxy_app_core::app_state::bare_app_state;
use tower::ServiceExt;

fn app() -> axum::Router {
    public_routes().with_state(bare_app_state())
}

#[tokio::test]
async fn the_host_resolved_health_read_is_mounted_and_refuses_an_admin_host() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/customer-apps/health")
                .header(header::HOST, "app.oxygen-hq.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "a host that names no custom app is told to use the slug-explicit route, \
         not asked for a credential"
    );
}

/// `debug` used to stay in `oxy-app`'s public tree. It moved into this crate
/// with the rest of the custom-app surface, and `oxy-app` no longer mounts it,
/// so this is now its only mount. With no database the handler answers (a 500
/// here), which a missing route never does.
#[tokio::test]
async fn debug_is_mounted_here() {
    for path in ["/customer-apps/acme/dash/debug"] {
        let response = app()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_ne!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{path} is a route of this crate, not a fallback 404"
        );
    }
}

#[tokio::test]
async fn a_route_this_crate_does_not_mount_is_a_404() {
    for path in [
        "/customer-apps/acme/dash/not-a-route",
        "/webhooks/apps/acme/dash",
    ] {
        let response = app()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
}

/// The tree the sandbox and promote tests drive is built from the seams once;
/// axum panics at build time on a path mounted twice (`Overlapping method route`),
/// which took every test on that fixture down. No database is needed to build it.
#[test]
fn the_flat_api_tree_builds_with_each_custom_app_route_mounted_once() {
    let _ = crate::custom_app_functions_fixture::flat_api_router();
}
