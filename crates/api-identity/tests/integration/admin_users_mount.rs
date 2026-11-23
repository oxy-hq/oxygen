//! The staff console's user directory is mounted, behind the capability it had
//! in `oxy-app` (admin split, PR 5). The served composition, with the staff
//! door and the capability layers, is driven in `oxy-server`'s
//! `served_router_tests`; this drives the section alone, nested at `/admin`
//! the way the admin seam mounts it.
//!
//! No auth setup: every handler here takes an `AuthenticatedUserExtractor`
//! (or the `RequestActor` built on it), which rejects before the handler body
//! runs, so a mounted route answers something other than 404 and an unmounted
//! path answers axum's 404. The controls at the end prove that discriminates.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

fn admin_tree() -> Router {
    Router::new()
        .nest("/admin", oxy_api_identity::admin_users_routes())
        .with_state(oxy_app::surface::bare_app_state())
}

async fn status(router: &Router, method: &str, path: &str) -> StatusCode {
    router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn the_user_directory_routes_are_mounted_under_admin() {
    let router = admin_tree();
    let user = "d9830be4-c6a4-4c1c-9c1e-000000000011";
    let org = "d9830be4-c6a4-4c1c-9c1e-000000000012";
    let invitation = "d9830be4-c6a4-4c1c-9c1e-000000000013";

    for (method, path) in [
        ("GET", "/admin/users".to_string()),
        ("GET", format!("/admin/users/{user}")),
        ("PATCH", format!("/admin/users/{user}/status")),
        ("GET", format!("/admin/users/{user}/org-memberships")),
        ("POST", format!("/admin/users/{user}/org-memberships")),
        // A membership write names the org in the path: the one the handler
        // fences before it touches the row.
        (
            "PATCH",
            format!("/admin/users/{user}/org-memberships/{org}"),
        ),
        (
            "DELETE",
            format!("/admin/users/{user}/org-memberships/{org}"),
        ),
        (
            "DELETE",
            format!("/admin/users/{user}/invitations/{invitation}"),
        ),
    ] {
        let got = status(&router, method, &path).await;
        assert_ne!(got, StatusCode::NOT_FOUND, "{method} {path} is not mounted");
        // TRACE, which no route here takes: 405 from a mounted path.
        let got = status(&router, "TRACE", &path).await;
        assert_eq!(
            got,
            StatusCode::METHOD_NOT_ALLOWED,
            "TRACE {path}: a mounted path answers 405"
        );
    }

    // Controls: a doubled prefix and a sibling nobody mounts both 404, so the
    // non-404s above are the routes, not a catch-all.
    assert_eq!(
        status(&router, "GET", "/admin/admin/users").await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        status(&router, "GET", "/admin/users-nobody-mounts").await,
        StatusCode::NOT_FOUND
    );
}

/// Exactly one section behind `PlatformUsers`, declaring nothing (the
/// console's `/{*rest}` FleetOk wildcard places it, as on `main`).
#[test]
fn the_user_directory_takes_platform_users() {
    use oxy_server_authz::Action;
    let found: Vec<_> = oxy_api_identity::admin_sections()
        .into_iter()
        .filter(|s| s.capability == Action::PlatformUsers)
        .collect();
    assert_eq!(found.len(), 1, "sections behind PlatformUsers");
    assert!(found[0].decls.is_empty(), "PlatformUsers declares nothing");
}
