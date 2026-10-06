//! "Run now" is audited for every credential, in the environment it queued
//! the run in (`admin::apps::run_audit`).
//!
//! Before, only a sandbox agent token's run left a row
//! (`sandbox_agent_token/the_loop.rs` still asserts that one). A staff
//! session and an app publish token — the two callers of the production
//! trigger — left none. A refused request queued nothing and leaves none.

use axum::Extension;
use axum::body::Bytes;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use entity::{app_publish_tokens, audit_events};
use oxy_app::server::api::admin::apps::environment_scope::EnvironmentQuery;
use oxy_app::server::api::admin::apps::handlers;
use oxy_auth::app_publish_token_domain::generate_token;
use oxy_auth::types::AppPublishTokenAuth;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde_json::json;
use uuid::Uuid;

use super::callers::staff;
use super::environment_checks::two_builds;
use super::post_admin;
use crate::custom_app_functions_fixture::{Tenant, seeded_tenant};

const RUN_QUEUED: &str = "app.function.run_queued";

async fn rows(t: &Tenant) -> Vec<audit_events::Model> {
    audit_events::Entity::find()
        .filter(audit_events::Column::Action.eq(RUN_QUEUED))
        .order_by_asc(audit_events::Column::Seq)
        .all(&t.db)
        .await
        .expect("read the audit rows")
}

/// A staff publish token of the guest's, as a row: `(marker, model)`.
async fn publish_token(
    t: &Tenant,
    app_id: Uuid,
) -> (AppPublishTokenAuth, app_publish_tokens::Model) {
    let token = generate_token();
    let row = app_publish_tokens::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        name: ActiveValue::Set("ci-checks".to_string()),
        token_hash: ActiveValue::Set(token.token_hash),
        token_prefix: ActiveValue::Set(token.token_prefix),
        created_by: ActiveValue::Set(Some(t.guest_id)),
        created_at: ActiveValue::Set(chrono::Utc::now().fixed_offset()),
        last_used_at: ActiveValue::Set(None),
        revoked_at: ActiveValue::Set(None),
        app_id: ActiveValue::Set(Some(app_id)),
        expires_at: ActiveValue::Set(None),
    }
    .insert(&t.db)
    .await
    .expect("mint the token");
    let marker = AppPublishTokenAuth {
        token_id: row.id,
        app_id: Some(app_id),
        machine_identity: None,
    };
    (marker, row)
}

/// `run_function_job` in production, as the router would call it for a
/// request the publish token behind `marker` authenticated.
async fn run_with_token(
    t: &Tenant,
    marker: &AppPublishTokenAuth,
    app_id: Uuid,
    function: &str,
) -> StatusCode {
    let outcome = handlers::run_function_job(
        oxy_app_core::audit::RequestActor::session(staff(t).0),
        Some(Extension(marker.clone())),
        Path((app_id, function.to_string())),
        Query(EnvironmentQuery { environment: None }),
        Bytes::new(),
    )
    .await;
    match outcome {
        Ok(_) => StatusCode::OK,
        Err(refused) => axum::response::IntoResponse::into_response(refused).status(),
    }
}

#[tokio::test]
async fn run_now_leaves_one_audit_row_for_a_session_and_for_a_publish_token() {
    let t = seeded_tenant().await;
    let app_id = two_builds(&t).await;
    let runs = format!("/apps/{app_id}/functions/smoke/runs");

    // The staff guest's own login: staging, then production.
    let (status, staged) = post_admin(&format!("{runs}?environment=staging")).await;
    assert_eq!(status, StatusCode::OK, "{staged}");
    let (status, live) = post_admin(&runs).await;
    assert_eq!(status, StatusCode::OK, "{live}");
    // A publish token: a declared check, in production.
    let (marker, token) = publish_token(&t, app_id).await;
    assert_eq!(
        run_with_token(&t, &marker, app_id, "smoke").await,
        StatusCode::OK
    );

    let written = rows(&t).await;
    assert_eq!(written.len(), 3, "one row per run queued");
    for (row, environment) in written.iter().zip(["staging", "production", "production"]) {
        assert_eq!(row.environment, environment);
        assert_eq!(row.org_id, Some(t.org_id));
        assert_eq!(row.actor_user_id, Some(t.guest_id));
        assert_eq!(row.target_type.as_deref(), Some("custom_app_function"));
        assert_eq!(row.target_id, Some(format!("{app_id}/smoke")));
        assert_eq!(row.metadata["function"], "smoke");
        assert!(
            row.metadata["run_id"]
                .as_str()
                .is_some_and(|id| !id.is_empty())
        );
    }
    assert_eq!(written[0].metadata["run_id"], staged["run_id"]);
    assert_eq!(written[1].metadata["run_id"], live["run_id"]);

    // A login is a user, and names no key.
    for row in &written[..2] {
        assert_eq!(row.actor_type, "user");
        assert!(row.metadata.get("token_id").is_none());
    }
    // The publish token is a key, named by id, label and display prefix.
    let by_token = &written[2];
    assert_eq!(by_token.actor_type, "api_key");
    assert_eq!(by_token.metadata["token_id"], json!(token.id));
    assert_eq!(by_token.metadata["token_kind"], "app_publish_token");
    assert_eq!(by_token.metadata["token_name"], "ci-checks");
    assert_eq!(
        by_token.metadata["display_prefix"],
        json!(token.token_prefix)
    );
    assert!(
        !format!("{by_token:?}").contains(&token.token_hash),
        "never the token's hash"
    );
}

#[tokio::test]
async fn a_refused_run_now_leaves_no_audit_row() {
    let t = seeded_tenant().await;
    let app_id = two_builds(&t).await;
    // `plain` is not a declared check: a publish token may not run it.
    let (marker, _) = publish_token(&t, app_id).await;
    assert_eq!(
        run_with_token(&t, &marker, app_id, "plain").await,
        StatusCode::FORBIDDEN
    );
    // Nor a check of an app it was not issued for.
    let elsewhere = AppPublishTokenAuth {
        token_id: Uuid::new_v4(),
        app_id: Some(Uuid::new_v4()),
        machine_identity: None,
    };
    assert_eq!(
        run_with_token(&t, &elsewhere, app_id, "smoke").await,
        StatusCode::FORBIDDEN
    );
    assert!(rows(&t).await.is_empty());
}
