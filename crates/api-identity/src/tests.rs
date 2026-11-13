use std::collections::HashSet;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use tower::ServiceExt;

use super::*;

/// These routes came from `oxy-app`, where `route_fleet`'s type proved the
/// role. Here the role is a declaration, so hold it: each path is DECLARED
/// FleetOk exactly once — not merely unclassified, which also reads FleetOk and
/// is what a dropped row would look like. Revoking a credential must never
/// wait on the one instance that can be down.
#[test]
fn every_route_is_declared_fleet_ok_exactly_once() {
    for (seam, decls) in [
        ("api", route_roles()),
        ("public", public_route_roles()),
        ("workspace", workspace_route_roles()),
    ] {
        let mut seen = HashSet::new();
        for decl in decls {
            assert!(
                seen.insert(decl.path),
                "{seam}: {} declared twice",
                decl.path
            );
            assert_eq!(decl.role, RouteRole::FleetOk, "{seam}: {}", decl.path);
            assert_eq!(decl.method, "*", "{seam}: {}", decl.path);
        }
    }
}

/// A declaration with no route behind it, or a route moved without its
/// declaration, would leave the two lists disagreeing. Each declared path is
/// mounted: TRACE, which no route here takes, answers 405 from a mounted path
/// and 404 from an unmounted one. Probed below the org layers, which would
/// answer first.
#[tokio::test]
async fn every_declared_path_is_mounted() {
    let state = oxy_app::surface::bare_app_state();
    let api = token_routes()
        .merge(Router::new().nest("/orgs/{org_id}", org_api_access_routes()))
        .with_state(state.clone());
    let public = public_routes().with_state(state.clone());
    let workspace = workspace_routes().with_state(state);
    for (seam, router, decls) in [
        ("api", api, route_roles()),
        ("public", public, public_route_roles()),
        ("workspace", workspace, workspace_route_roles()),
    ] {
        for decl in decls {
            let uri = concrete(decl.path);
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::TRACE)
                        .uri(&uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "{seam}: {} is declared but not mounted",
                decl.path
            );
        }
    }
}

/// A declared path with every `{param}` filled in.
fn concrete(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            if segment.starts_with('{') {
                "00000000-0000-0000-0000-000000000001"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// The two staff sections keep the capabilities they had in-tree.
#[test]
fn the_admin_sections_keep_their_capabilities() {
    let caps: Vec<Action> = admin_sections().iter().map(|s| s.capability).collect();
    assert_eq!(caps, vec![Action::PlatformOperate, Action::PlatformGrants]);
}

/// The five legacy key operations stay in the served OpenAPI document.
#[test]
fn the_legacy_key_operations_are_documented() {
    let doc = openapi();
    for (path, methods) in [
        ("/{workspace_id}/api-keys", &["get", "post"][..]),
        ("/{workspace_id}/api-keys/{id}", &["get", "delete"][..]),
        ("/{workspace_id}/api-keys/{id}/extend", &["post"][..]),
    ] {
        let item = doc
            .paths
            .paths
            .get(path)
            .unwrap_or_else(|| panic!("{path} is missing"));
        for method in methods {
            let op = match *method {
                "get" => &item.get,
                "post" => &item.post,
                "delete" => &item.delete,
                _ => unreachable!(),
            };
            assert!(op.is_some(), "{method} {path} is missing");
        }
    }
}
