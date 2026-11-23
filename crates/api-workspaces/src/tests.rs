//! The surface's own contract: what it mounts, what it declares, and that the
//! two agree. The served router's mount of it is checked where the router is
//! composed, in `oxy-server`'s `served_router_tests`.

use super::*;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use oxy_app_core::pods::role_manifest::declared_role_in;
use tower::ServiceExt;

/// Every concrete workspace route, with path parameters filled.
const WORKSPACE_MOUNTED: &[&str] = &[
    "/meta",
    "/members",
    "/members/65c121b2-0000-0000-0000-000000000000",
    "/oxy-access",
    "/app-integrations",
];

/// Every concrete org route, with path parameters filled.
const ORG_MOUNTED: &[&str] = &[
    "/orgs/11111111-1111-1111-1111-111111111111/workspaces",
    "/orgs/11111111-1111-1111-1111-111111111111/workspaces/22222222-2222-2222-2222-222222222222",
    "/orgs/11111111-1111-1111-1111-111111111111/workspaces/22222222-2222-2222-2222-222222222222/rename",
];

async fn trace(router: &Router, uri: &str) -> StatusCode {
    router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::TRACE)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// Each row is declared, FleetOk and method `*`, exactly once.
fn assert_fleet_ok_exactly_once(declared: &[RouteRoleDecl]) {
    for decl in declared {
        let same = declared
            .iter()
            .filter(|d| d.method == decl.method && d.path == decl.path)
            .count();
        assert_eq!(
            same, 1,
            "{} {}: declared {same} times",
            decl.method, decl.path
        );
        assert_eq!(decl.role, RouteRole::FleetOk, "{}", decl.path);
        assert_eq!(decl.method, "*", "{}", decl.path);
    }
}

/// The routes came from `oxy-app`, where `route_fleet` wrote these rows. Here
/// the role is a declaration, so hold it: each row is DECLARED FleetOk exactly
/// once — not merely unclassified, which `classify` also answers FleetOk and
/// is what a dropped row would look like.
#[test]
fn every_workspace_route_is_declared_fleet_ok_exactly_once() {
    let declared = workspace_route_roles();
    assert_eq!(
        declared.len(),
        5,
        "the metadata, the members and one member's override, the Oxy-access \
         switch and the app integrations"
    );
    assert_fleet_ok_exactly_once(declared);
}

#[test]
fn every_org_route_is_declared_fleet_ok_exactly_once() {
    let declared = route_roles();
    assert_eq!(declared.len(), 3, "the list, a delete and a rename");
    assert_fleet_ok_exactly_once(declared);
}

/// The declarations hold against the role classifier the way the served router
/// uses them: prefixed by the workspace tree, then matched against a live URI.
/// Every mounted route must match a declaration, not the default.
#[test]
fn every_mounted_route_classifies_fleet_ok() {
    let ws: Vec<_> = workspace_route_roles()
        .iter()
        .map(|d| (d.method, format!("/api/{{workspace_id}}{}", d.path), d.role))
        .collect();
    for path in WORKSPACE_MOUNTED {
        let uri = format!("/api/d9830be4-c6a4{path}");
        for method in ["GET", "POST", "PUT", "DELETE"] {
            assert_eq!(
                declared_role_in(&ws, method, &uri),
                Some(RouteRole::FleetOk),
                "{method} {uri} must match a FleetOk declaration"
            );
        }
    }
    let org: Vec<_> = route_roles()
        .iter()
        .map(|d| (d.method, format!("/api{}", d.path), d.role))
        .collect();
    for path in ORG_MOUNTED {
        let uri = format!("/api{path}");
        for method in ["GET", "PATCH", "DELETE"] {
            assert_eq!(
                declared_role_in(&org, method, &uri),
                Some(RouteRole::FleetOk),
                "{method} {uri} must match a FleetOk declaration"
            );
        }
    }
}

/// The rows `oxy-app`'s `role_manifest_tests` (`ide_routes_classify_against_live_uri`)
/// and `tests/routing/global_route_roles.rs` (`deleting_a_workspace_stays_on_the_fleet`,
/// the list row of `org_reads_stay_on_the_fleet`) asked of `classify` there,
/// asked of this crate's declarations here, so a dropped row fails rather than
/// taking the FleetOk default.
///
/// The metadata is Postgres. Deleting a workspace removes the row, its
/// schedules and its Airhouse tenant record, and touches files only where the
/// serving process owns them, so a fleet replica answers it whole. The LIST
/// must stay on the fleet: viewing workspaces cannot require the singleton.
#[test]
fn persisted_data_routes_are_declared_fleet_ok() {
    let ws: Vec<_> = workspace_route_roles()
        .iter()
        .map(|d| (d.method, format!("/api/{{workspace_id}}{}", d.path), d.role))
        .collect();
    assert_eq!(
        declared_role_in(&ws, "GET", "/api/d9830be4-c6a4/meta"),
        Some(RouteRole::FleetOk)
    );
    let org: Vec<_> = route_roles()
        .iter()
        .map(|d| (d.method, format!("/api{}", d.path), d.role))
        .collect();
    for (method, path) in [
        ("DELETE", "/api/orgs/some-org/workspaces/d9830be4-c6a4"),
        ("GET", "/api/orgs/some-org/workspaces"),
        (
            "DELETE",
            "/api/orgs/11111111-1111-1111-1111-111111111111/workspaces/22222222-2222-2222-2222-222222222222",
        ),
        (
            "GET",
            "/api/orgs/11111111-1111-1111-1111-111111111111/workspaces",
        ),
    ] {
        assert_eq!(
            declared_role_in(&org, method, path),
            Some(RouteRole::FleetOk),
            "{method} {path} is Postgres only"
        );
    }
}

/// A declaration with no route behind it (or a route moved without its
/// declaration) leaves the two lists disagreeing. TRACE, which no route here
/// takes, answers 405 from a mounted path and 404 from an unmounted one,
/// without running a handler.
#[tokio::test]
async fn every_workspace_route_is_mounted() {
    let router = workspace_routes().with_state(oxy_app_core::app_state::bare_app_state());
    for uri in WORKSPACE_MOUNTED {
        assert_eq!(
            trace(&router, uri).await,
            StatusCode::METHOD_NOT_ALLOWED,
            "{uri}: not mounted"
        );
    }
    assert_eq!(
        trace(&router, "/nothing-mounts-this").await,
        StatusCode::NOT_FOUND,
        "control"
    );
}

/// The org routes, without `org_middleware` (which reads a database) and the
/// subscription guard; the served router's tests mount them with both.
#[tokio::test]
async fn every_org_route_is_mounted() {
    let router = Router::new()
        .nest("/orgs/{org_id}", org_handlers())
        .with_state(oxy_app_core::app_state::bare_app_state());
    for uri in ORG_MOUNTED {
        assert_eq!(
            trace(&router, uri).await,
            StatusCode::METHOD_NOT_ALLOWED,
            "{uri}: not mounted"
        );
    }
    assert_eq!(
        trace(
            &router,
            "/orgs/11111111-1111-1111-1111-111111111111/nothing"
        )
        .await,
        StatusCode::NOT_FOUND,
        "control"
    );
}

/// The two operations `oxy-app`'s curated document listed, each still carrying
/// its method. A `routes!(..)` line compiles whether or not its handler keeps a
/// `#[utoipa::path]`, so only the document can show a loss.
#[test]
fn the_documented_operations_are_the_two_the_curated_document_listed() {
    let doc = openapi();
    let paths = &doc.paths.paths;
    for path in ["/workspaces/{workspace_id}", "/orgs/{org_id}/workspaces"] {
        assert!(
            paths.get(path).is_some_and(|item| item.get.is_some()),
            "GET {path} is not documented"
        );
    }
    assert_eq!(paths.len(), 2, "{:?}", paths.keys().collect::<Vec<_>>());
}
