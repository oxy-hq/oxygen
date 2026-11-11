//! The slug-addressed invocation read: `GET /customer-apps/{org}/{app}/invocations`.
//!
//! The `/logs` gate, not the staff console's:
//!
//! - the tenant's own app admin (the org Owner, not Oxy staff) reads
//!   production's invocations, which the console's `{id}/invocations` never
//!   showed them, and is refused a non-production environment's;
//! - naming no environment reads every environment for staff who may open
//!   non-production ones, and production's for everyone else — as the
//!   console's listing does — and `environments` says which;
//! - a `?build=` only a non-production environment ran is asked about as that
//!   environment, and read across environments once admitted;
//! - rows carry metadata, not a stored response body;
//! - an unknown app is the resolver's `404`.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use oxy_api_custom_apps::logs::invocations as slug_invocations;
use serde_json::Value;
use tower::ServiceExt;

use super::answer;
use super::readback::{APP, OTHER_APP, PRODUCTION_BUILD, STAGING_BUILD, ids, ran, two_builds};
use crate::custom_app_functions_fixture::{Tenant, seeded_tenant};
use crate::staging_functions::{make_guest_staff, production_host, staging_host};

/// The route as `public_routes` mounts it (below `/api`): inline
/// authentication, no router guard. `custom_app_functions_manual_run_guards`
/// checks production still mounts this handler on this path.
pub(crate) const SLUG_INVOCATIONS_ROUTE: &str = "/customer-apps/{org_slug}/{app_slug}/invocations";

/// `GET …/invocations<query>` for `app`, as the guest.
async fn read(t: &Tenant, app: &str, query: &str) -> (StatusCode, Value) {
    let uri = format!("/customer-apps/{}/{app}/invocations{query}", t.org_slug);
    let request = Request::get(uri).body(Body::empty()).expect("request");
    let response = Router::new()
        .route(
            SLUG_INVOCATIONS_ROUTE,
            get(slug_invocations::get_invocations),
        )
        .oneshot(request)
        .await
        .expect("oneshot");
    answer(response).await
}

#[tokio::test]
async fn the_apps_admin_reads_production_and_staff_read_one_environment_at_a_time() {
    let t = seeded_tenant().await;
    two_builds(&t, APP, PRODUCTION_BUILD, STAGING_BUILD).await;
    two_builds(&t, OTHER_APP, "rb-other-prod", "rb-other-stg").await;

    // The guest is the org's Owner — the app's admin, not Oxy staff.
    let live = ran(&t, APP, "whoami", &production_host(&t, APP))
        .await
        .to_string();
    for query in ["", "?environment=production", "?environment="] {
        let (status, body) = read(&t, APP, query).await;
        assert_eq!(status, StatusCode::OK, "{query:?}: {body}");
        // Without non-production reach, naming nothing is production alone,
        // and the response says so.
        assert_eq!(body["environments"], "production", "{query:?}: {body}");
        let rows = &body["invocations"];
        assert_eq!(ids(rows), vec![live.clone()], "{query:?}");
        let row = &rows[0];
        assert_eq!(row["function_name"], "whoami");
        assert_eq!(row["environment"], "production");
        assert_eq!(row["build_id"], PRODUCTION_BUILD);
        assert_eq!(row["mode"], "route");
        // Metadata only: whether a body was kept, never the body.
        assert!(row.get("has_result").is_some(), "{row}");
        assert!(row.get("result_body").is_none(), "{row}");
        assert!(row.get("held").is_none(), "{row}");
    }
    for query in ["?environment=staging", "?environment=dev-a1"] {
        let (status, body) = read(&t, APP, query).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{query}: {body}");
        assert_eq!(body["error"], "non_production_refused", "{query}: {body}");
    }
    // A build only staging runs names staging, as on the console: refused,
    // not an empty list read from production. Production's build is not.
    let (status, body) = read(&t, APP, &format!("?build={STAGING_BUILD}")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"], "non_production_refused", "{body}");
    let (status, body) = read(&t, APP, &format!("?build={PRODUCTION_BUILD}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(ids(&body["invocations"]), vec![live.clone()]);
    let (status, body) = read(&t, APP, "?environment=Staging").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "invalid_environment");
    let (status, body) = read(&t, APP, "?limit=0").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "invalid_limit");
    let (status, body) = read(&t, "no-such-app", "").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // Staff with non-production reach. Another app: the reach decision above
    // is cached per (user, app).
    make_guest_staff();
    let other_live = ran(&t, OTHER_APP, "whoami", &production_host(&t, OTHER_APP))
        .await
        .to_string();
    let other_staged = ran(&t, OTHER_APP, "whoami", &staging_host(&t, OTHER_APP))
        .await
        .to_string();
    // Naming one environment reads that one.
    let (status, body) = read(&t, OTHER_APP, "?environment=staging").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(ids(&body["invocations"]), vec![other_staged.clone()]);
    assert_eq!(body["environments"], "staging");
    // A non-production build: admitted as staging, read across environments.
    let (status, body) = read(&t, OTHER_APP, "?build=rb-other-stg").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(ids(&body["invocations"]), vec![other_staged.clone()]);
    assert_eq!(body["environments"], "all");
    // Naming nothing is every environment for staff with reach, newest first.
    let (status, body) = read(&t, OTHER_APP, "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(ids(&body["invocations"]), vec![other_staged, other_live]);
    assert_eq!(body["environments"], "all");
}
