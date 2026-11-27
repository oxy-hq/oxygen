//! The custom-app Sentry surface tag that the **gate** sets: a data-plane call
//! the URL rules cannot recognise is tagged by `enter_semantic_boundary`, so a
//! custom app's errors still reach Sentry under the custom-app surface.
//!
//! Moved here from `oxy-app`'s `tests/routing/sentry_hub_scan.rs` in decompose
//! R-a (2026-11-24), into `oxy-workspace-runtime`'s integration group, and on
//! with the gate into this crate in depth stage C (A2, 2026-11-27). These two
//! tests drive `oxy_api_custom_apps::custom_apps_gates::enter_semantic_boundary`,
//! the real gate, so they need this crate's graph; the URL-rule tests, the
//! plain platform call and the source walks stayed with the text scans.
//! `route_surface_tag` is a copy of the one beside those: the plain-call test
//! there still uses it.

use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::middleware::Next;
use oxy_telemetry::sentry_filter::{CUSTOM_APP_SURFACE, CUSTOM_APP_SURFACE_TAG};
use oxy_telemetry::sentry_surface::tag_custom_app_surface;
use sentry::integrations::tower::NewSentryLayer;
use tower::{ServiceBuilder, ServiceExt};

/// Serve one request through the **real** custom-app gate —
/// `enter_semantic_boundary`, the door every semantic-analysis twin enters
/// by, and through it `check_custom_app_gates` — behind the two layers
/// serve.rs installs, and return the tag on an event captured later in the
/// same request. (It mounted `/query`, then the metric tree's
/// `post_baseline`, until each moved to `oxy-api-custom-apps`; the handler
/// here is one line around the same entry point, so the gate that tags is
/// the shipped one.)
///
/// The `Origin` is deliberately foreign, so `check_custom_app_gates` stops
/// at its allowlist step and the test needs no database. The gate tags
/// before it authenticates, so that is exactly the case the tag must
/// already cover.
fn data_plane_surface_tag(host: &str) -> Option<String> {
    route_surface_tag(
        Router::new().route(
            "/api/projects/{project_id}/semantic/metric-tree/baseline",
            axum::routing::post(
                |axum::extract::Path(project_id): axum::extract::Path<uuid::Uuid>,
                 headers: axum::http::HeaderMap| async move {
                    use axum::response::IntoResponse as _;
                    match oxy_api_custom_apps::custom_apps_gates::enter_semantic_boundary(
                        &headers, project_id,
                    )
                    .await
                    {
                        Ok(_) => axum::http::StatusCode::OK.into_response(),
                        Err(response) => response,
                    }
                },
            ),
        ),
        host,
        "/api/projects/0195f0d2-0000-7000-8000-000000000000/semantic/metric-tree/baseline",
    )
}

/// As [`data_plane_surface_tag`], for any router. The capture happens in a
/// layer *inside* `NewSentryLayer`, so it lands on the request's own hub —
/// standing in for the handler's `error!`, an `OxyError::capture_to_sentry`
/// or a panic later in the same request.
fn route_surface_tag(app: Router, host: &str, path: &str) -> Option<String> {
    let events = sentry::test::with_captured_events(|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let service = ServiceBuilder::new()
                .layer(NewSentryLayer::<Request>::new_from_top())
                // The shipped URL rule is in the stack on purpose: these
                // URLs are the ones it cannot recognise, so whatever tag
                // appears came from the gate.
                .layer(axum::middleware::from_fn(tag_custom_app_surface))
                .layer(axum::middleware::from_fn(
                    |req: Request, next: Next| async move {
                        let response = next.run(req).await;
                        sentry::capture_message("boom", sentry::Level::Error);
                        response
                    },
                ))
                .service(app);
            let request = Request::builder()
                .method("POST")
                .uri(path)
                .header("host", host)
                // Not this host, and not a loopback pair: `is_allowed_origin`
                // refuses it, so the gate returns 403 before any DB call.
                .header("origin", "https://not-this-host.example")
                .body(Body::empty())
                .expect("request");
            service.oneshot(request).await.expect("response");
        });
    });
    assert_eq!(events.len(), 1, "exactly one captured event");
    events[0].tags.get(CUSTOM_APP_SURFACE_TAG).cloned()
}

/// C1. The canonical app URL is `<base>/customer-apps/<org>/<app>/`, and the
/// bundle fetches its data same-origin with an empty `apiBaseUrl` — so the
/// data call lands on the platform host with an `/api` path. No URL rule can
/// see that; the gate can.
#[test]
fn a_custom_app_data_call_on_the_platform_host_is_tagged_by_the_gate() {
    assert_eq!(
        data_plane_surface_tag("app.oxygen-hq.com"),
        Some(CUSTOM_APP_SURFACE.to_string()),
        "a custom app's warehouse query must not reach Sentry untagged"
    );
}

/// The same call from an app served on an org subdomain. `org_host_dispatch`
/// passes `/api/**` through untouched by design, so this request is
/// indistinguishable from a platform one by host or path too.
#[test]
fn a_custom_app_data_call_on_an_org_subdomain_is_tagged_by_the_gate() {
    assert_eq!(
        data_plane_surface_tag("acme.oxygen-hq.com"),
        Some(CUSTOM_APP_SURFACE.to_string())
    );
}
