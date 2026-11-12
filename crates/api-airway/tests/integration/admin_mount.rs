//! The staff console's airway section is mounted, and behind the capability it
//! had in `oxy-app`. Moved from `oxy-app`'s `admin::tests::
//! airway_config_is_mounted_on_the_real_router` with the section.
//!
//! No auth setup: every handler here takes an `AuthenticatedUserExtractor`,
//! which rejects before the handler body runs, so a mounted route answers
//! something other than 404 and an unmounted path answers axum's 404. That is
//! the discriminator, and the bogus sibling path at the end is the control
//! that proves it discriminates.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

fn admin_tree() -> Router {
    Router::new()
        .nest("/admin", oxy_api_airway::admin_routes())
        .with_state(oxy_app::surface::bare_app_state())
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
        assert_ne!(
            status,
            StatusCode::NOT_FOUND,
            "{method} {path} must be mounted by `admin_routes()` — got 404"
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
        oxy_app::surface::Action::PlatformOperate
    ));
}
