//! A trailing slash must not change which pod serves a route.
//!
//! `nest_service` registers the prefix WITH a trailing slash; `nest` does not
//! (measured: `nest_service` answers `/p/` with 200, `nest` answers it 404). So
//! when `/modeling` moved to `nest_service` to carry its own state, `GET
//! /modeling/` went from a 404 to reaching `list_projects` — which reads the
//! working copy — while classifying FleetOk, because no route pattern carries a
//! trailing slash.
//!
//! `classify` trims it now, so this holds for every prefix, not just that one.

use oxy_app::server::role_manifest::{RouteRole, classify, install_route_declarations_for_tests};

#[test]
fn a_trailing_slash_keeps_the_route_on_its_own_pod() {
    install_route_declarations_for_tests();
    let ws = "d9830be4-c6a4-4f89-11d3-9a0c0305e82c";
    for path in [
        format!("/api/{ws}/modeling"),
        format!("/api/{ws}/modeling/"),
        format!("/api/{ws}/modeling/projects"),
        format!("/api/{ws}/files"),
        format!("/api/{ws}/files/"),
    ] {
        assert_eq!(
            classify("GET", &path),
            RouteRole::IdeOnly,
            "{path} reads the working copy and must reach the ide"
        );
    }
}

/// The trim must not swallow the root, which is a real path.
#[test]
fn the_root_path_still_classifies() {
    install_route_declarations_for_tests();
    assert_eq!(classify("GET", "/"), RouteRole::FleetOk);
}

/// The behaviour the guard above depends on, pinned so it cannot drift with an
/// axum upgrade: only `nest_service` answers the trailing slash.
#[tokio::test]
async fn only_nest_service_serves_the_trailing_slash() {
    use axum::routing::get;
    use tower::ServiceExt;

    async fn status(router: axum::Router<()>, uri: &str) -> u16 {
        router
            .oneshot(
                axum::http::Request::builder()
                    .uri(uri)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
            .as_u16()
    }

    let inner = || axum::Router::<()>::new().route("/", get(|| async { "root" }));
    let nested = axum::Router::<()>::new().nest("/p", inner());
    let serviced =
        axum::Router::<()>::new().nest_service("/p", inner().into_service::<axum::body::Body>());

    assert_eq!(status(nested.clone(), "/p").await, 200);
    assert_eq!(status(nested, "/p/").await, 404, "nest does not serve it");
    assert_eq!(status(serviced.clone(), "/p").await, 200);
    assert_eq!(
        status(serviced, "/p/").await,
        200,
        "nest_service does — which is why nest_typed must declare it",
    );
}

/// The last three routes that used to live in a hand-written table. None of
/// them is a route in the protected router, so each found a different home:
/// `/ide*` is a URL prefix the middleware owns (no handler exists — it is the
/// SPA `fallback_service`), and the custom-app routes are declared by the
/// module whose one handler serves them.
#[test]
fn the_routes_that_have_no_mount_still_classify() {
    install_route_declarations_for_tests();

    // Bundle bytes come from S3; `POST .../fn/<name>` executes a function
    // against the promoted revision. One handler, any pod.
    assert_eq!(
        classify("POST", "/customer-apps/acme/dash/fn/refresh"),
        RouteRole::FleetOk,
    );
    assert_eq!(
        classify("GET", "/customer-apps/acme/dash/assets/main.js"),
        RouteRole::FleetOk,
    );
}
