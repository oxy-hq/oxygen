//! The staff console's per-org data-plane sections, OLTP and Airhouse, are
//! mounted, each behind the capability it had in `oxy-app`. Moved from
//! `oxy-app`'s `admin::tests::{oltp_admin_routes_are_mounted_on_the_real_router,
//! airhouse_admin_routes_are_mounted_on_the_real_router}` with the sections
//! (admin split, PR 4). The served composition, with the staff door and the
//! capability layers, is driven in `oxy-server`'s `served_router_tests`.
//!
//! No auth setup: every handler here takes an `AuthenticatedUserExtractor`
//! (or the `RequestActor` built on it), which rejects before the handler body
//! runs, so a mounted route answers something other than 404 and an unmounted
//! path answers axum's 404. That is the discriminator, and the controls at the
//! end of each test prove it discriminates.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

/// The sections the way the admin seam mounts them: relative to `/admin`.
fn admin_tree() -> Router {
    Router::new()
        .nest(
            "/admin",
            oxy_api_tenancy::admin_oltp_routes().merge(oxy_api_tenancy::admin_airhouse_routes()),
        )
        .with_state(oxy_app::surface::bare_app_state())
}

/// The same guard for the per-org OLTP routes, which shipped 404ing.
///
/// They were declared as `/admin/orgs/{id}/oltp` while `admin::router()` is
/// already nested under `/admin` in `router::global` — so the real path was
/// `/admin/admin/orgs/...`. It compiled, the handler was reachable in
/// principle, and every request 404'd. Nothing but driving the real router
/// catches a doubled prefix.
#[tokio::test]
async fn oltp_admin_routes_are_mounted_on_the_real_router() {
    let real_router = admin_tree();
    let org = "d9830be4-c6a4-4c1c-9c1e-000000000002";

    for (method, path) in [
        // All seven, not the original three. The three that matter most to
        // get wrong sit together: visibility grants analytics read access to
        // a tenant's live tables, `deprovision-writer` drops one app's
        // schema, and the DELETE destroys the whole database. The last two
        // are one path segment apart and differ by the entire tenant, which
        // is why both are pinned here rather than trusted to review.
        // `GET /oltp` additionally changed crates when the scope-fenced shim
        // took over the mount.
        ("GET", "/admin/oltp".to_string()),
        ("GET", format!("/admin/orgs/{org}/oltp")),
        ("POST", format!("/admin/orgs/{org}/oltp/test-connection")),
        ("POST", format!("/admin/orgs/{org}/oltp/provision")),
        ("POST", format!("/admin/orgs/{org}/oltp/credentials")),
        ("POST", format!("/admin/orgs/{org}/oltp/visibility")),
        ("POST", format!("/admin/orgs/{org}/oltp/deprovision-writer")),
        ("DELETE", format!("/admin/orgs/{org}/oltp")),
        // The staging branch: its status, and the reset that discards every
        // app's staging data in the org.
        ("GET", format!("/admin/orgs/{org}/oltp/branches/staging")),
        (
            "POST",
            format!("/admin/orgs/{org}/oltp/branches/staging/reset"),
        ),
        // Airhouse rides the same mount and would 404 the same way.
        ("GET", "/admin/airhouse".to_string()),
        (
            "POST",
            format!("/admin/workspaces/{org}/airhouse/provision"),
        ),
        (
            "POST",
            format!("/admin/workspaces/{org}/airhouse/rotate-sa"),
        ),
    ] {
        let req = Request::builder()
            .method(method)
            .uri(&path)
            .body(Body::empty())
            .unwrap();
        let resp = real_router.clone().oneshot(req).await.unwrap();
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "{method} {path} must be mounted by the section — a 404 here means the \
             merge is missing or the route carries its own /admin prefix"
        );
    }

    // Control: the doubled-prefix shape must NOT match, or the assertions
    // above would pass for the very bug they exist to catch.
    let doubled = Request::builder()
        .uri(format!("/admin/admin/orgs/{org}/oltp").as_str())
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        real_router.oneshot(doubled).await.unwrap().status(),
        StatusCode::NOT_FOUND,
        "the /admin/admin/... shape must not be mounted"
    );
}

/// The Airhouse admin routes, driven through the section's real router.
///
/// A 404 on a real path means the route is not mounted; the bogus sibling
/// below stays 404 as the control.
#[tokio::test]
async fn airhouse_admin_routes_are_mounted_on_the_real_router() {
    let real_router = admin_tree();
    let ws = "3c6e0b8a-9c15-224a-8236-000000000001";

    for (method, path) in [
        ("GET", "/admin/airhouse".to_string()),
        ("POST", format!("/admin/workspaces/{ws}/airhouse/provision")),
        ("POST", format!("/admin/workspaces/{ws}/airhouse/rotate-sa")),
    ] {
        let req = Request::builder()
            .method(method)
            .uri(&path)
            .body(Body::empty())
            .unwrap();
        let resp = real_router.clone().oneshot(req).await.unwrap();
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "{method} {path} is not mounted"
        );
    }

    // The control: an unmounted sibling must still 404, or the assertions
    // above would pass for the very bug they exist to catch.
    let bogus = Request::builder()
        .uri("/admin/airhouse/not-a-route")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        real_router.oneshot(bogus).await.unwrap().status(),
        StatusCode::NOT_FOUND,
        "an unmounted path must 404, otherwise this test proves nothing"
    );
}

/// Each section names the capability `admin::router` gave it in `oxy-app`,
/// and `oxy-app` applies the guard for the capability a section names
/// (`admin::extracted_sections`, tested there). A different capability would
/// silently widen or narrow who reaches the console — an App Operator must not
/// provision a billable database — so both are pinned, each on its own routes.
#[test]
fn the_data_plane_sections_take_their_own_capabilities() {
    use oxy_app::surface::Action;
    let sections = oxy_api_tenancy::admin_sections();
    for capability in [Action::PlatformOltp, Action::PlatformAirhouse] {
        let found: Vec<_> = sections
            .iter()
            .filter(|s| s.capability == capability)
            .collect();
        assert_eq!(found.len(), 1, "sections behind {capability:?}");
        assert!(found[0].decls.is_empty(), "{capability:?} declares nothing");
    }
}
