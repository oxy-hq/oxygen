//! `PUT /customer-apps/{id}/promotion-checks`: what it stores, answers,
//! audits and refuses.

use axum::Extension;
use axum::Json;
use axum::extract::Path;
use axum::response::IntoResponse;
use oxy_api_custom_apps::admin::apps::promotion_checks::set_promotion_checks;
use oxy_app_core::agent_refusal::RefuseSandboxAgent;
use oxy_auth::types::AppPublishTokenAuth;

use super::*;
use crate::promote_rollback::audit_rows;
use crate::sandbox_agent_token::fixture::{agent, app_model, send};

const CHANGED: &str = "app.promotion_checks_changed";
const DEFAULTS: [&str; 4] = ["functions", "loads", "design_system", "smoke"];

async fn put(agent: &Agent, body: Value) -> (StatusCode, Value) {
    let uri = format!("/customer-apps/{}/promotion-checks", agent.app.id);
    person(agent, "PUT", &uri, Some(body)).await
}

#[tokio::test]
async fn the_settings_are_stored_answered_on_the_app_and_audited_once() {
    let agent = agent().await;
    let uri = format!("/customer-apps/{}", agent.app.id);
    let (status, app) = person(&agent, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK, "{app}");
    let defaults = json!({
        "enabled": false, "required": null, "effective_required": DEFAULTS,
        "seen_required": false, "production_smoke": true, "auto_rollback": false,
    });
    assert_eq!(app["promotion_checks"], defaults, "{app}");

    // Stored whether or not the gate is on; the list held to list order.
    let asked = json!({
        "required": ["smoke", "loads", "smoke"], "seen_required": true,
        "production_smoke": false, "auto_rollback": true,
    });
    let (status, app) = put(&agent, asked.clone()).await;
    assert_eq!(status, StatusCode::OK, "{app}");
    let stored = json!({
        "enabled": false, "required": ["loads", "smoke"],
        "effective_required": ["loads", "smoke"], "seen_required": true,
        "production_smoke": false, "auto_rollback": true,
    });
    assert_eq!(app["promotion_checks"], stored, "{app}");
    assert_eq!(app["id"], json!(agent.app.id), "the whole app: {app}");
    let (_, read_back) = person(&agent, "GET", &uri, None).await;
    assert_eq!(read_back["promotion_checks"], stored, "{read_back}");

    let audited = audit_rows(&agent, CHANGED).await;
    assert_eq!(audited.len(), 1, "{audited:?}");
    assert_eq!(audited[0].actor_user_id, Some(agent.t.guest_id));
    assert_eq!(audited[0].target_id, Some(agent.app.id.to_string()));
    let kept = json!({
        "required": ["loads", "smoke"], "seen_required": true,
        "production_smoke": false, "auto_rollback": true,
    });
    let before = json!({
        "required": null, "seen_required": false,
        "production_smoke": true, "auto_rollback": false,
    });
    assert_eq!(audited[0].before, Some(before), "the defaults it replaced");
    assert_eq!(audited[0].after, Some(kept), "what was stored");
    // Sending the same again changes nothing and writes no second row.
    let (status, _) = put(&agent, asked).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(audit_rows(&agent, CHANGED).await.len(), 1);

    // The flag turns `enabled` on, and `required: null` is the defaults again.
    enable_gate(&agent).await;
    let (status, app) = put(&agent, json!({ "required": null })).await;
    assert_eq!(status, StatusCode::OK, "{app}");
    let on = json!({
        "enabled": true, "required": null, "effective_required": DEFAULTS,
        "seen_required": false, "production_smoke": true, "auto_rollback": false,
    });
    assert_eq!(app["promotion_checks"], on, "{app}");
    assert_eq!(audit_rows(&agent, CHANGED).await.len(), 2);
}

#[tokio::test]
async fn a_kind_that_cannot_be_required_is_refused_saying_why() {
    let agent = agent().await;
    for (kind, says) in [
        ("validation", "validation_not_passed"),
        ("changes", "seen_required"),
        ("production_smoke", "not a check an app can require"),
        ("nonsense", "not a check an app can require"),
    ] {
        let (status, refused) = put(&agent, json!({ "required": ["loads", kind] })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{kind}: {refused}");
        assert_eq!(refused["code"], "invalid_required", "{kind}: {refused}");
        let message = refused["message"].as_str().unwrap_or_default();
        assert!(message.contains(says), "{kind}: {message}");
    }
    let (status, refused) = person(
        &agent,
        "PUT",
        &format!("/customer-apps/{}/promotion-checks", Uuid::new_v4()),
        Some(json!({ "required": [] })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
    assert_eq!(
        app_model(&agent.t.db, agent.app.id).await.promotion_checks,
        None
    );
    assert!(audit_rows(&agent, CHANGED).await.is_empty());
}

/// The settings decide what CI's builds must pass, and CI's token is what
/// they hold: a publish token is refused by the route's own layers, and by
/// the handler for any credential that carries the marker.
#[tokio::test]
async fn a_publish_token_cannot_set_the_settings() {
    let agent = agent().await;
    let minted = Some(json!({ "name": "ci" }));
    let (status, minted) = person(&agent, "POST", "/admin/app-publish-tokens", minted).await;
    assert_eq!(status, StatusCode::OK, "mint a publish token: {minted}");
    let token = minted["token"].as_str().expect("the token");
    let uri = format!("/customer-apps/{}/promotion-checks", agent.app.id);
    let bearer = format!("Bearer {token}");
    let body = json!({ "required": [] });
    let (status, refused) = send(
        "PUT",
        &uri,
        &[("authorization", &bearer)],
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");

    let marker = Extension(AppPublishTokenAuth {
        token_id: Uuid::new_v4(),
        app_id: Some(agent.app.id),
        machine_identity: None,
    });
    let asked = serde_json::from_value(body).expect("the settings");
    let refused = set_promotion_checks(
        RefuseSandboxAgent,
        agent.t.guest(),
        Some(marker),
        Path(agent.app.id),
        Json(asked),
    )
    .await
    .expect_err("a publish token's settings")
    .into_response();
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    let bytes = axum::body::to_bytes(refused.into_body(), 64 * 1024)
        .await
        .expect("a body");
    let refused: Value = serde_json::from_slice(&bytes).expect("JSON");
    assert_eq!(refused["code"], "publish_token_refused", "{refused}");

    assert_eq!(
        app_model(&agent.t.db, agent.app.id).await.promotion_checks,
        None
    );
    assert!(audit_rows(&agent, CHANGED).await.is_empty());
}
