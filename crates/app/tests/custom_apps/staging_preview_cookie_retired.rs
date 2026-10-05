//! The staff draft-preview cookie is retired: the draft is served on the app's
//! staging host only (custom-app staging from the console, Task 1).
//!
//! `oxy_preview_draft=1` used to flip the **production** host to the draft for a
//! staff viewer — draft HTML only, with production's functions behind it, which
//! is the mismatch the staging environment exists to remove. Now:
//!
//! - `POST`/`DELETE /api/customer-apps/preview-draft` no longer exist, so
//!   nothing can set or clear the cookie;
//! - a production request still carrying it — the path URL on the admin host,
//!   or the production subdomain — is served the **published** build, staff or
//!   not (its data-plane pin is `custom_app_staging_pin`'s to prove);
//! - the staging host still serves the staging (draft) build to staff, with no
//!   cookie at all;
//! - the debug snapshot (`GET /api/customer-apps/{org}/{app}/debug`) makes the
//!   same decision: `draft` on the staging host, `published` on production even
//!   with the cookie.
//!
//! The route check drives the real `api_router`, so this module is pinned into
//! `serial-db` in `.config/nextest.toml` (see `authz::shared_db_registry`).

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use axum::routing::get;
use oxy_app::server::api::custom_apps_debug;
use oxy_app::server::router::api_router;
use oxy_app_core::serve_mode::ServeMode;
use tower::ServiceExt;

use crate::app_environments_phase_1b::{
    APP, make_guest_staff, production_host, staging_host, two_environments,
};
use crate::custom_app_functions_fixture::{ORG_SLUG, seeded_tenant, serve_router};

/// The retired cookie, exactly as `custom_apps_preview` used to set it.
const PREVIEW_COOKIE: &str = "oxy_preview_draft=1";

/// The app's HTML, on `host` (`None` = the admin host's path URL), with or
/// without the retired cookie. No session cookie: the zero-config guest is the
/// viewer, made Oxy staff by `make_guest_staff`.
async fn get_html(host: Option<&str>, cookie: bool) -> (StatusCode, String) {
    let mut request = Request::builder()
        .uri(format!("/customer-apps/{ORG_SLUG}/{APP}/"))
        .header(header::ACCEPT, "text/html");
    if let Some(host) = host {
        request = request.header(header::HOST, host);
    }
    if cookie {
        request = request.header(header::COOKIE, PREVIEW_COOKIE);
    }
    let request = request.body(Body::empty()).expect("request");
    let response = serve_router().oneshot(request).await.expect("oneshot");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn build_marker(build: uuid::Uuid) -> String {
    format!("\"buildId\":\"{build}\"")
}

#[tokio::test]
async fn serve_ignores_the_preview_cookie_on_production_and_staging_serves_the_draft() {
    let t = seeded_tenant().await;
    let (_, production_build, staging_build) = two_environments(&t).await;
    make_guest_staff();

    // The console's old Draft view: the path URL (and the production
    // subdomain), staff, cookie set. Both now serve what customers see.
    let production = production_host();
    for host in [None, Some(production.as_str())] {
        let (status, html) = get_html(host, true).await;
        assert_eq!(status, StatusCode::OK, "{host:?}: {html}");
        assert!(
            html.contains(&build_marker(production_build)),
            "{host:?}: production serves the published build despite {PREVIEW_COOKIE}: {html}"
        );
        assert!(
            !html.contains(&build_marker(staging_build)),
            "{host:?}: the draft never reaches the production host: {html}"
        );
        assert!(html.contains("\"environment\":\"production\""), "{html}");
    }

    // The draft moved to the staging host, and needs no cookie there.
    let (status, html) = get_html(Some(&staging_host()), false).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(
        html.contains(&build_marker(staging_build)),
        "the staging host serves the draft: {html}"
    );
    assert!(html.contains("\"environment\":\"staging\""), "{html}");
}

/// `POST`/`DELETE /api/customer-apps/preview-draft` are gone. The path is
/// shadowed by `/customer-apps/{id}` (GET/PATCH/DELETE), so an authorised
/// staff caller gets that route's refusal rather than a bare 404: `405` for the
/// POST (no POST on `{id}`), `400` for the DELETE (`preview-draft` is not a
/// UUID). Pinned exactly, so a route that starts answering here again shows up
/// as a changed status. Neither sets or clears the cookie.
#[tokio::test]
async fn staging_preview_draft_route_is_gone() {
    let _t = seeded_tenant().await;
    make_guest_staff();
    let (router, _external, _preagg) = api_router(
        ServeMode::Cloud,
        false,
        None,
        std::path::PathBuf::new(),
        tokio_util::sync::CancellationToken::new(),
        true,
        oxy_app::server::router::SurfaceSeams::empty(),
    )
    .await
    .expect("build router");

    for (method, expected) in [
        (Method::POST, StatusCode::METHOD_NOT_ALLOWED),
        (Method::DELETE, StatusCode::BAD_REQUEST),
    ] {
        let request = Request::builder()
            .method(method.clone())
            .uri("/customer-apps/preview-draft")
            .body(Body::empty())
            .expect("request");
        let response = router.clone().oneshot(request).await.expect("oneshot");
        let status = response.status();
        let sets_cookie = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .any(|v| v.contains("oxy_preview_draft"));
        assert_eq!(
            status, expected,
            "{method} /api/customer-apps/preview-draft is no longer a route"
        );
        assert!(!sets_cookie, "{method} must not touch oxy_preview_draft");
    }
}

/// The debug snapshot for `host` (`None` = the admin host), with or without the
/// retired cookie, mounted exactly as `router/public.rs` mounts it under `/api`.
async fn get_debug(host: Option<&str>, cookie: bool) -> serde_json::Value {
    let router = Router::new().route(
        "/customer-apps/{org_slug}/{app_slug}/debug",
        get(custom_apps_debug::get_debug),
    );
    let mut request = Request::builder().uri(format!("/customer-apps/{ORG_SLUG}/{APP}/debug"));
    if let Some(host) = host {
        request = request.header(header::HOST, host);
    }
    if cookie {
        request = request.header(header::COOKIE, PREVIEW_COOKIE);
    }
    let request = request.body(Body::empty()).expect("request");
    let response = router.oneshot(request).await.expect("oneshot");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("body");
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).expect("debug snapshot is JSON")
}

#[tokio::test]
async fn debug_route_reports_draft_on_staging_and_published_on_production_with_the_cookie() {
    let t = seeded_tenant().await;
    let (_, production_build, staging_build) = two_environments(&t).await;
    // Oxy staff: may open the app's staging (`may_open_non_production`).
    make_guest_staff();

    let staging = get_debug(Some(&staging_host()), false).await;
    assert_eq!(staging["channel"], "draft", "{staging}");
    assert_eq!(staging["build"], staging_build.to_string(), "{staging}");

    let production = production_host();
    for host in [None, Some(production.as_str())] {
        let snap = get_debug(host, true).await;
        assert_eq!(
            snap["channel"], "published",
            "{host:?}: {PREVIEW_COOKIE} no longer flips the snapshot to the draft: {snap}"
        );
        assert_eq!(
            snap["build"],
            production_build.to_string(),
            "{host:?}: {snap}"
        );
    }
}
