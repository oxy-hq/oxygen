//! The staff console's airway section is mounted, and behind the capability it
//! had in `oxy-app`. Moved from `oxy-app`'s `admin::tests::
//! airway_config_is_mounted_on_the_real_router` with the section.
//!
//! **No handler body runs here, and that is load-bearing.** A route layer
//! answers every *matched* request with [`ROUTED`] before its handler is
//! called, and an unmatched path still falls through to axum's 404. That pair
//! is the discriminator, and the bogus sibling path at the end is the control
//! that proves it discriminates.
//!
//! The test used to send each request to the real handler, on the premise that
//! every one takes an `AuthenticatedUserExtractor` and so rejects before its
//! body. Five do not: the global-row and deployment-config handlers rely on the
//! section's `PlatformOperate` guard, which `oxy-server` layers on and this
//! bare router does not. The two `PUT`s were saved by their `Json` extractor
//! refusing the empty body, and the `GET` only reads, but the two `DELETE`s
//! take nothing that can refuse. So `DELETE /admin/airway/config/toast` really
//! ran `delete_global` against `OXY_DATABASE_URL`, and
//! `DELETE /admin/airway/deployment-config` really deleted the singleton
//! deployment row: in CI, the same shared Postgres that
//! `admin::handlers::tests` and `admin::deployment::tests` serialize on with an
//! advisory lock this test never took. A `handlers::tests` case that wrote the
//! global `toast` row and read it back between those two moments found it gone
//! (`upsert_replaces_rather_than_duplicating_the_global_row`: 0 rows, not 1).

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::{self, Next};
use tower::ServiceExt;

/// What a matched route answers here instead of running its handler. Not a
/// status any airway handler or extractor returns, so seeing it means the
/// router matched the path. Only the path: axum wraps a route's
/// method-not-allowed fallback in the same layer, so this is no stronger on
/// methods than the `!= 404` check it replaced, and no weaker.
const ROUTED: StatusCode = StatusCode::IM_A_TEAPOT;

/// Stops a matched request before its handler, so the handler cannot reach
/// the database. Never calls `next`.
async fn stop_before_the_handler(_req: Request<Body>, _next: Next) -> StatusCode {
    ROUTED
}

fn admin_tree() -> Router {
    Router::new()
        .nest("/admin", oxy_api_airway::admin_routes())
        // `route_layer`, not `layer`: it wraps matched routes only, so an
        // unregistered path still reaches the router's own 404.
        .route_layer(middleware::from_fn(stop_before_the_handler))
        .with_state(oxy_app_core::app_state::bare_app_state())
}

#[tokio::test]
async fn every_airway_admin_route_is_mounted() {
    let router = admin_tree();
    let ws = "d9830be4-c6a4-4c1c-9c1e-000000000001";

    for (method, path) in [
        ("GET", "/admin/airway/config".to_string()),
        ("PUT", "/admin/airway/config/toast".to_string()),
        ("DELETE", "/admin/airway/config/toast".to_string()),
        ("PUT", format!("/admin/airway/config/toast/workspaces/{ws}")),
        (
            "DELETE",
            format!("/admin/airway/config/toast/workspaces/{ws}"),
        ),
        ("GET", "/admin/airway/config/toast/preview".to_string()),
        // The operational tier: a sibling of `/config`, so it is also the case
        // where a bad nest shape would have `/config/{source_kind}` match it.
        ("GET", "/admin/airway/deployment-config".to_string()),
        ("PUT", "/admin/airway/deployment-config".to_string()),
        ("DELETE", "/admin/airway/deployment-config".to_string()),
    ] {
        let req = Request::builder()
            .method(method)
            .uri(&path)
            .body(Body::empty())
            .unwrap();
        let status = router.clone().oneshot(req).await.unwrap().status();
        assert_eq!(
            status, ROUTED,
            "{method} {path} must be mounted by `admin_routes()` — got {status}"
        );
    }

    let bogus = Request::builder()
        .uri("/admin/airway/does-not-exist")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        router.oneshot(bogus).await.unwrap().status(),
        StatusCode::NOT_FOUND,
        "control: an unregistered sibling path must 404, or the loop above \
         discriminates nothing"
    );
}

/// The section names `PlatformOperate`, and `oxy-app` applies the guard for the
/// capability a section names (`admin::extracted_sections`, tested there). A
/// different capability here would silently widen or narrow who reaches the
/// console, so it is pinned.
#[test]
fn the_section_takes_platform_operate() {
    let sections = oxy_api_airway::admin_sections();
    assert_eq!(sections.len(), 1);
    assert!(matches!(
        sections[0].capability,
        oxy_server_authz::Action::PlatformOperate
    ));
}
