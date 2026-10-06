//! The held-write read-back: `GET /admin/apps/{id}/invocations/{inv}/held`.
//!
//! - it returns what a staging call held and `[]` for a production call;
//! - **nothing reads across apps, orgs, invocations or audit actions**:
//!   another app's invocation id is the same `404` as an unknown one, and a
//!   row that names the invocation from another org, names another invocation
//!   in this one, or is not a held-write row at all is never returned;
//! - a caller who may not open the app's non-production environments cannot
//!   read a staging invocation's held list.

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use entity::users::UserStatus;
use oxy_app::server::api::admin::apps::held_writes;
use oxy_app_core::audit::{AuditEntry, record_best_effort};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AuthenticatedUser;
use serde_json::{Value, json};
use uuid::Uuid;

use super::readback::{APP, OTHER_APP, PRODUCTION_BUILD, STAGING_BUILD, ran, two_builds};
use super::{answer, get_admin};
use crate::custom_app_functions_fixture::{Tenant, seeded_tenant, throwaway_org};
use crate::staging_functions::{make_guest_staff, production_host, staging_host};

/// A held row for `invocation`, written into `org` by hand.
async fn forge_held_row(t: &Tenant, org: Uuid, invocation: Uuid, op: &str) {
    forge_audit_row(t, org, invocation, "app.staging.held", op).await;
}

/// An audit row of `action` in `org` whose metadata names `invocation` and
/// lists one write — the shape a held row and the write-audit rows
/// (`data_audit::entry`) share.
async fn forge_audit_row(t: &Tenant, org: Uuid, invocation: Uuid, action: &'static str, op: &str) {
    record_best_effort(
        &t.db,
        AuditEntry::new("system:app:forged".to_string(), action)
            .org(org)
            .environment("staging")
            .metadata(json!({
                "invocation_id": invocation,
                "writes": [{ "op": op, "plane": "forged" }],
            })),
    )
    .await;
}

#[tokio::test]
async fn the_held_route_returns_one_invocations_held_writes_and_no_one_elses() {
    let t = seeded_tenant().await;
    let app_id = two_builds(&t, APP, PRODUCTION_BUILD, STAGING_BUILD).await;
    let other_app = two_builds(&t, OTHER_APP, "rb-other-prod", "rb-other-stg").await;
    make_guest_staff();

    let staged = ran(&t, APP, "writes", &staging_host(&t, APP)).await;
    let live = ran(&t, APP, "writes", &production_host(&t, APP)).await;
    let quiet = ran(&t, APP, "whoami", &staging_host(&t, APP)).await;
    let theirs = ran(&t, OTHER_APP, "writes", &staging_host(&t, OTHER_APP)).await;
    let held = |app: Uuid, invocation: Uuid| async move {
        get_admin(&format!("/apps/{app}/invocations/{invocation}/held")).await
    };
    let ops = |body: &Value| -> Vec<String> {
        body["held"]
            .as_array()
            .unwrap_or_else(|| panic!("held: {body}"))
            .iter()
            .map(|w| w["op"].as_str().expect("op").to_string())
            .collect()
    };

    // Rows that must never be read: one naming this invocation from another
    // org, and one in this org naming a different invocation.
    let stranger = throwaway_org(&t).await;
    forge_held_row(&t, stranger.org_id, staged, "forged.other-org").await;
    forge_held_row(&t, t.org_id, Uuid::new_v4(), "forged.other-invocation").await;
    // And rows that are not held writes at all: the write-audit rows carry the
    // same metadata shape, in this org, for this very invocation.
    for action in ["app.oltp.write", "app.warehouse.write"] {
        forge_audit_row(&t, t.org_id, staged, action, "forged.other-action").await;
    }

    let (status, body) = held(app_id, staged).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        (
            &body["invocation_id"],
            &body["environment"],
            &body["function"],
            &body["build_id"],
            &body["status"]
        ),
        (
            &json!(staged),
            &json!("staging"),
            &json!("writes"),
            &json!(STAGING_BUILD),
            &json!("success")
        ),
        "{body}"
    );
    assert_eq!(
        ops(&body),
        vec!["fetch"],
        "what staging held, and only that"
    );

    // Production held nothing; a staging call that wrote nothing held nothing.
    let (status, body) = held(app_id, live).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        (&body["environment"], ops(&body)),
        (&json!("production"), vec![])
    );
    let (_, body) = held(app_id, quiet).await;
    assert_eq!(ops(&body), Vec::<String>::new());

    // Another app's invocation, read through this app's path, is the same 404
    // as an id that does not exist — and the same from the other side.
    for (app, invocation) in [
        (app_id, theirs),
        (other_app, staged),
        (app_id, Uuid::new_v4()),
    ] {
        let (status, body) = held(app, invocation).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{app}/{invocation}: {body}");
        assert_eq!(body["error"], "invocation_not_found");
        assert!(body.get("held").is_none(), "{body}");
    }
    let (status, body) = held(Uuid::new_v4(), staged).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "app_not_found");

    // A caller who may not open the app's non-production environments reads a
    // production invocation's (empty) list and is refused a staging one's.
    let outsider = || {
        AuthenticatedUserExtractor(AuthenticatedUser {
            id: Uuid::new_v4(),
            email: Some("tenant-admin@customer.example".to_string()),
            name: "Outsider".to_string(),
            picture: None,
            status: UserStatus::Active,
            credential: None,
        })
    };
    let refused = held_writes::get_held_writes(outsider(), None, Path((app_id, staged)))
        .await
        .map(|_| ())
        .expect_err("a staging invocation's held list is staff's");
    let (status, body) = answer(refused.into_response()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"], "non_production_refused");
    assert!(body.get("held").is_none(), "{body}");
    let axum::Json(allowed) = held_writes::get_held_writes(outsider(), None, Path((app_id, live)))
        .await
        .expect("a production invocation holds nothing to hide");
    assert!(allowed.held.is_empty());
}
