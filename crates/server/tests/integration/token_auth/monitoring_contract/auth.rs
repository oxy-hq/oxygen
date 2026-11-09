//! Which credentials ferret's four routes accept, and how they refuse.
//!
//! **ferret depends on this; changing it needs a matching change in
//! oxy-hq/infrastructure `services/ferret`** (`staffClientFrom` and the
//! `x-api-key` header in `src/uptime/staff-client.ts`).
//!
//! ferret sends one key, in `x-api-key`, on every request. It reads `401` and
//! `403` as "the key was not accepted" — UNKNOWN, never an alert — so a route
//! that stops accepting the key it holds goes quiet rather than red. Pinned
//! here per route: every key it can hold (its service token, and the staff
//! keys it falls back to) is accepted; no key, an unknown key, a key whose owner is not staff, and a
//! staff user's token without platform standing are refused with the statuses
//! ferret tells apart.

use axum::http::StatusCode;

use super::super::seed_user;
use super::super::stack::{Reach, mint, published_app};
use super::app_health::health_uri;
use super::{ferret_get, query, staff_fixture, staff_keys};

/// Ferret's four reads, as it sends them, for an app of the fixture's org.
fn ferret_reads(health: &str) -> Vec<(&'static str, String)> {
    let page = query(&[("limit", "200"), ("offset", "0")]);
    let until = chrono::Utc::now();
    let window = query(&[
        ("since", &(until - chrono::Duration::days(1)).to_rfc3339()),
        ("until", &until.to_rfc3339()),
        ("limit", "500"),
    ]);
    vec![
        ("the app list", format!("/customer-apps?{page}")),
        (
            "fleet-health",
            format!("/customer-apps/fleet-health?{page}"),
        ),
        ("one app's health", health.to_string()),
        (
            "the function-failure feed",
            format!("/customer-apps/function-failures?{window}"),
        ),
    ]
}

#[tokio::test]
async fn a_staff_key_in_x_api_key_is_accepted_on_every_route() {
    let fx = staff_fixture().await;
    let app = published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    let reads = ferret_reads(&health_uri(fx.org_id, &app));

    for (shape, key) in staff_keys(&fx).await {
        for (route, uri) in &reads {
            let answer = ferret_get(Some(&key), uri).await;
            // The app has no build, so its health is a verdict, not a refusal:
            // 503 with a body. Every other route is a 200.
            let accepted = if *route == "one app's health" {
                answer.status == StatusCode::SERVICE_UNAVAILABLE
                    && answer.body["oxy_app_health"] == "fail"
            } else {
                answer.status == StatusCode::OK
            };
            assert!(
                accepted,
                "{route} refused ferret's {shape}: {} {}",
                answer.status, answer.body
            );
        }
    }
}

#[tokio::test]
async fn every_route_refuses_what_is_not_a_staff_key_with_401_or_403() {
    let fx = staff_fixture().await;
    let app = published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    let outsider = seed_user(&fx.db, "outsider").await;
    let (_, outsider_key) = mint(&fx.db, outsider.id, Reach::all_access()).await;
    // The staff user's own token, minted without platform standing.
    let (_, standingless) = mint(&fx.db, fx.user.id, Reach::all_access()).await;
    let unknown = format!("oxy_{}", uuid::Uuid::new_v4().simple());

    let refusals: [(&str, Option<&str>, StatusCode); 4] = [
        ("no key", None, StatusCode::UNAUTHORIZED),
        ("an unknown key", Some(&unknown), StatusCode::UNAUTHORIZED),
        (
            "a key whose owner is not staff",
            Some(&outsider_key),
            StatusCode::FORBIDDEN,
        ),
        (
            "a staff token without platform standing",
            Some(&standingless),
            StatusCode::FORBIDDEN,
        ),
    ];
    for (route, uri) in ferret_reads(&health_uri(fx.org_id, &app)) {
        for (credential, key, expected) in refusals {
            let answer = ferret_get(key, &uri).await;
            assert_eq!(
                answer.status, expected,
                "{route} with {credential}: ferret reads {expected} as 'the key was not \
                 accepted' (UNKNOWN); got {} {}",
                answer.status, answer.body
            );
        }
    }
}
