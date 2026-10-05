//! The router `serve` builds, from the seams this composition root hands it.
//!
//! `oxy-app` cannot depend on the surface crates (the dependency runs the other
//! way), so its own router tests build `api_router` with `SurfaceSeams::empty()`.
//! Once #3460 moved `/orgs` into `oxy-api-tenancy`, an assertion there that cloud
//! mode serves `/orgs` could only fail, and its local-mode mirror could only
//! pass. Both halves — `api_router` mounting the api seam in cloud mode and
//! dropping it in local mode, and this crate putting tenancy on that seam — are
//! visible only here, so the pair lives here.
//!
//! The routes under one org, and the GitHub namespace routes that hang off the
//! same prefix from `oxy-api-github`, came over from `oxy-app`'s
//! `local_mode_router` for the same reason, with one more: that file asked for
//! `/organizations…` and `/github/namespaces…`, which no crate registers in any
//! mode, so its 404s said nothing about either surface.
//!
//! # Why the org id in these paths is not a UUID
//!
//! "Mounted" is read off "not the router's 404", so every probe has to be a
//! request whose only possible 404 IS the router's. A well-formed org id is not
//! one: with no auth configured the built-in authenticator attaches the guest
//! user, the request reaches `org_middleware`, and that answers 404 for an org
//! that does not exist — the status of a route nobody mounted. An id the
//! middleware cannot parse is rejected (400) before anything is looked up, so
//! with [`ORG`] a mounted route is never 404 and an unmounted one always is.
//!
//! Each helper also asserts its control first — an unmounted path is 404 in
//! cloud mode, the local router serves `/live` — so if either stops holding the
//! control fails, instead of the assertion passing for nothing.
//!
//! `api_router` connects to `OXY_DATABASE_URL` while it builds, and runs the
//! unscoped `cleanup_stale_runs` over `agentic_runs` — so these skip without a
//! database, and `.config/nextest.toml` pins this module into `serial-db`.

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use oxy_app::server::router::api_router;
use oxy_app_core::serve_mode::ServeMode;
use tower::ServiceExt;

use crate::surface_seams;

/// An org id no route can parse. See the module docs.
const ORG: &str = "not-a-uuid";

/// A path under the same prefix that no crate registers.
const UNMOUNTED: &str = "/orgs-nothing-mounts-this";

fn db_unavailable() -> bool {
    std::env::var("OXY_DATABASE_URL").is_err()
}

/// `oxy-api-tenancy`'s org tree: the list, and the routes under one org.
fn organization_routes() -> Vec<(Method, String)> {
    vec![
        (Method::GET, "/orgs".to_string()),
        (Method::GET, format!("/orgs/{ORG}")),
        (Method::GET, format!("/orgs/{ORG}/members")),
        (Method::GET, format!("/orgs/{ORG}/invitations")),
    ]
}

/// `oxy-api-github`'s namespace routes, under the same org prefix from a
/// different crate.
fn github_namespace_routes() -> Vec<(Method, String)> {
    vec![
        (Method::GET, format!("/orgs/{ORG}/github/namespaces")),
        (Method::POST, format!("/orgs/{ORG}/github/namespaces/pat")),
    ]
}

/// The router `serve` builds in `mode`, from the seams boot hands it.
async fn served(mode: ServeMode) -> Router {
    let (router, _external_router, _preagg) = api_router(
        mode,
        false,
        None,
        std::path::PathBuf::new(),
        tokio_util::sync::CancellationToken::new(),
        false,
        surface_seams(),
    )
    .await
    .expect("router built");
    router
}

async fn status(router: &Router, method: &Method, path: &str) -> StatusCode {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    router.clone().oneshot(req).await.expect("oneshot").status()
}

/// Cloud mode serves every one of `routes`: each gets some answer other than
/// the 404 a path nobody mounts gets from the same router.
async fn assert_cloud_mounts(routes: Vec<(Method, String)>) {
    let router = served(ServeMode::Cloud).await;
    assert_eq!(
        status(&router, &Method::GET, UNMOUNTED).await,
        StatusCode::NOT_FOUND,
        "control: an unmounted path must be 404, or a non-404 below proves nothing"
    );
    for (method, path) in routes {
        let got = status(&router, &method, &path).await;
        assert_ne!(
            got,
            StatusCode::NOT_FOUND,
            "cloud mode must keep {method} {path} mounted, got {got}"
        );
    }
}

/// Local mode serves none of `routes`: it never mounts the org tree, so it must
/// drop the api seam they ride — while still serving its own routes.
async fn assert_local_drops(routes: Vec<(Method, String)>) {
    let router = served(ServeMode::Local).await;
    assert_eq!(
        status(&router, &Method::GET, "/live").await,
        StatusCode::OK,
        "control: the local router must serve something, or a 404 below proves nothing"
    );
    for (method, path) in routes {
        let got = status(&router, &method, &path).await;
        assert_eq!(
            got,
            StatusCode::NOT_FOUND,
            "local mode must not serve {method} {path}, got {got}"
        );
    }
}

#[tokio::test]
async fn cloud_mode_mounts_the_organizations_surface() {
    if db_unavailable() {
        return;
    }
    assert_cloud_mounts(organization_routes()).await;
}

#[tokio::test]
async fn local_mode_drops_the_organizations_surface() {
    if db_unavailable() {
        return;
    }
    assert_local_drops(organization_routes()).await;
}

#[tokio::test]
async fn cloud_mode_mounts_the_github_namespace_surface() {
    if db_unavailable() {
        return;
    }
    assert_cloud_mounts(github_namespace_routes()).await;
}

#[tokio::test]
async fn local_mode_drops_the_github_namespace_surface() {
    if db_unavailable() {
        return;
    }
    assert_local_drops(github_namespace_routes()).await;
}
