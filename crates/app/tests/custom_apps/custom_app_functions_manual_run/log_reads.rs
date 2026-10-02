//! The log read: `GET /customer-apps/{org}/{app}/logs`.
//!
//! - it takes `?environment=`, and a non-production environment's lines are
//!   staff's: the tenant's own app admin reads production's and is refused
//!   staging's;
//! - an `invocation_id` or `request_id` that is not a UUID is a `400` before
//!   anything reaches the store.
//!
//! No ClickHouse here, so a read that passes every gate answers `501` — what
//! is under test is who, and what, gets that far.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use oxy_app::server::api::custom_apps_logs;
use serde_json::Value;
use tower::ServiceExt;

use super::answer;
use super::readback::{APP, OTHER_APP, PRODUCTION_BUILD, STAGING_BUILD, two_builds};
use crate::custom_app_functions_fixture::{Tenant, seeded_tenant};
use crate::staging_functions::make_guest_staff;

/// The log route as `router/public.rs` mounts it (below `/api`): inline
/// authentication, no router guard. `custom_app_functions_manual_run_guards`
/// checks production still mounts this handler on this path.
pub(crate) const LOGS_ROUTE: &str = "/customer-apps/{org_slug}/{app_slug}/logs";
/// Its sibling, the client-error read, mounted the same way.
pub(crate) const ERRORS_ROUTE: &str = "/customer-apps/{org_slug}/{app_slug}/errors";

/// `GET …/<read><query>` for `app`, as the guest; `read` is `logs` or `errors`.
async fn read(t: &Tenant, app: &str, read: &str, query: &str) -> (StatusCode, Value) {
    let uri = format!("/customer-apps/{}/{app}/{read}{query}", t.org_slug);
    let request = Request::get(uri).body(Body::empty()).expect("request");
    let response = Router::new()
        .route(LOGS_ROUTE, get(custom_apps_logs::get_logs))
        .route(ERRORS_ROUTE, get(custom_apps_logs::get_errors))
        .oneshot(request)
        .await
        .expect("oneshot");
    answer(response).await
}

/// `GET …/logs<query>` for `app`, as the guest.
async fn logs(t: &Tenant, app: &str, query: &str) -> (StatusCode, Value) {
    read(t, app, "logs", query).await
}

/// A UUID, as every id these reads filter by is.
const AN_ID: &str = "0b7a1c2e-5d3f-4a6b-8c9d-0e1f2a3b4c5d";

/// The id filters arrive from the query string and once reached the store's
/// SQL as typed: a lone backslash swallowed the quote that closed its
/// literal, and the next filter ran as SQL. Anything that is not a UUID is
/// now refused by the route, for the app's own admin, before the store —
/// which this test does not run, so a filter that gets past answers `501`.
#[tokio::test]
async fn a_log_or_error_filter_that_is_not_an_id_is_a_400() {
    let t = seeded_tenant().await;
    two_builds(&t, APP, PRODUCTION_BUILD, STAGING_BUILD).await;

    let refused = [
        ("logs", "?invocation_id=%5C", "invocation_id"),
        ("logs", "?invocation_id=not-a-uuid", "invocation_id"),
        (
            "logs",
            "?invocation_id=%5C&request_id=%20OR%201%3D1%20--%20",
            "invocation_id",
        ),
        ("logs", "?request_id=%5C", "request_id"),
        (
            "logs",
            "?request_id=%27%20OR%20%271%27%3D%271",
            "request_id",
        ),
        ("errors", "?build_id=%5C", "build_id"),
        (
            "errors",
            "?build_id=x%5C%27%20OR%201%3D1%20--%20",
            "build_id",
        ),
    ];
    for (route, query, parameter) in refused {
        let (status, body) = read(&t, APP, route, query).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{route}{query}: {body}");
        assert_eq!(body["error"], "invalid_filter", "{route}{query}: {body}");
        assert_eq!(
            body["message"],
            format!("{parameter} must be a UUID"),
            "{route}{query}"
        );
    }

    // An id, an absent filter and a blank one all get as far as the store.
    let admitted = [
        ("logs", format!("?invocation_id={AN_ID}&request_id={AN_ID}")),
        ("logs", "?invocation_id=&request_id=".to_string()),
        ("logs", String::new()),
        ("errors", format!("?build_id={AN_ID}")),
        ("errors", String::new()),
    ];
    for (route, query) in admitted {
        let (status, body) = read(&t, APP, route, &query).await;
        assert_eq!(
            status,
            StatusCode::NOT_IMPLEMENTED,
            "{route}{query}: {body}"
        );
    }
}

#[tokio::test]
async fn a_non_production_environments_logs_are_staff_only() {
    let t = seeded_tenant().await;
    two_builds(&t, APP, PRODUCTION_BUILD, STAGING_BUILD).await;
    two_builds(&t, OTHER_APP, "rb-other-prod", "rb-other-stg").await;

    // The guest is the org's Owner — the app's admin, and not Oxy staff.
    // Production's lines are theirs: the read passes its gates and stops at
    // the store this test does not run.
    for query in ["", "?environment=production", "?environment="] {
        let (status, body) = logs(&t, APP, query).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{query:?}: {body}");
    }
    // A non-production environment's lines are not.
    for query in ["?environment=staging", "?environment=dev-a1"] {
        let (status, body) = logs(&t, APP, query).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{query}: {body}");
        assert_eq!(body["error"], "non_production_refused", "{query}: {body}");
    }
    let (status, body) = logs(&t, APP, "?environment=Staging").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "invalid_environment");

    // Oxy staff who may open the app's non-production environments read them.
    // Another app: the reach decision above is cached per (user, app).
    make_guest_staff();
    for query in ["?environment=staging", "?environment=dev-a1", ""] {
        let (status, body) = logs(&t, OTHER_APP, query).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{query:?}: {body}");
    }
}
