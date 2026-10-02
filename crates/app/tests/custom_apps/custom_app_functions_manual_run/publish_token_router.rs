//! A real publish token, through real authentication, on the surface a token
//! may read: `/api/customer-apps/{id}/…`.
//!
//! `invocation_reach` and `run_readback` hand the handlers a marker built by
//! hand. Here the token is a row in `app_publish_tokens` and arrives as a
//! bearer: `auth_middleware` resolves it and stamps the marker, the token's
//! scope middleware admits the `GET`, the mount's guards pass its minter, and
//! the handler must still refuse it every non-production row — while the same
//! requests on the minter's own login are answered.

use agentic_pipeline::scheduler::enqueue_app_function_job_in;
use axum::http::StatusCode;
use entity::app_publish_tokens;
use oxy_auth::app_publish_token_domain::generate_token;
use sea_orm::{ActiveModelTrait, ActiveValue};
use serde_json::Value;
use uuid::Uuid;

use super::get_customer_apps;
use super::readback::{APP, PRODUCTION_BUILD, STAGING_BUILD, ids, ran, two_builds};
use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{Tenant, seeded_tenant};
use crate::staging_functions::{make_guest_staff, production_host, staging_host};

/// A staff publish token minted by the guest: the bearer to present.
async fn minted_token(t: &Tenant) -> String {
    let token = generate_token();
    app_publish_tokens::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        name: ActiveValue::Set("ci".to_string()),
        token_hash: ActiveValue::Set(token.token_hash),
        token_prefix: ActiveValue::Set(token.token_prefix),
        created_by: ActiveValue::Set(Some(t.guest_id)),
        created_at: ActiveValue::Set(chrono::Utc::now().fixed_offset()),
        last_used_at: ActiveValue::Set(None),
        revoked_at: ActiveValue::Set(None),
        app_id: ActiveValue::Set(None),
        expires_at: ActiveValue::Set(None),
    }
    .insert(&t.db)
    .await
    .expect("mint the token");
    token.plaintext
}

#[tokio::test]
async fn a_real_publish_token_is_refused_every_non_production_read_through_the_router() {
    let t = seeded_tenant().await;
    let app_id = two_builds(&t, APP, PRODUCTION_BUILD, STAGING_BUILD).await;
    make_guest_staff();
    let live = ran(&t, APP, "whoami", &production_host(&t, APP))
        .await
        .to_string();
    let staged = ran(&t, APP, "whoami", &staging_host(&t, APP))
        .await
        .to_string();
    let run = enqueue_app_function_job_in(
        &t.db,
        &app_id.to_string(),
        "whoami",
        demo_workspace_id(),
        None,
        "manual",
        None,
        None,
        Some("staging"),
    )
    .await
    .expect("queue a staging run");
    let token = minted_token(&t).await;
    let token = Some(token.as_str());

    let listing = format!("/{app_id}/functions/whoami/invocations");
    let run_detail = format!("/{app_id}/function-runs/{run}");

    // Naming a non-production environment, or a build only one serves: refused.
    let build = format!("?build={STAGING_BUILD}");
    for named in ["?environment=staging", build.as_str()] {
        let (status, body) = get_customer_apps(&format!("{listing}{named}"), token).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{named}: {body}");
        assert_eq!(body["error"], "publish_token_refused", "{named}: {body}");
    }
    // Naming nothing: production's rows, and no staging row.
    let (status, body) = get_customer_apps(&listing, token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(ids(&body), vec![live.clone()], "{body}");
    // A non-production run by its id: the bare not-found of an unknown id.
    let (status, body) = get_customer_apps(&run_detail, token).await;
    assert_eq!((status, body), (StatusCode::NOT_FOUND, Value::Null));

    // The token's minter, on their own login, reads all of it.
    let (status, body) = get_customer_apps(&format!("{listing}?environment=staging"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(ids(&body), vec![staged.clone()], "{body}");
    let (_, body) = get_customer_apps(&listing, None).await;
    assert_eq!(ids(&body), vec![staged, live], "{body}");
    let (status, body) = get_customer_apps(&run_detail, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["environment"], "staging");
}
