//! `override_checks`: who may, with what reason, what it writes, and what it
//! does not clear.

use axum::Extension;
use axum::body::Bytes;
use axum::extract::Path;
use axum::response::IntoResponse;
use entity::app_builds;
use oxy_api_custom_apps::admin::apps::handlers as admin;
use oxy_app_core::agent_refusal::RefuseSandboxAgent;
use oxy_auth::types::AppPublishTokenAuth;
use sea_orm::sea_query::Expr;

use super::*;
use crate::promote_rollback::{audit_rows, draft, fixture_builds, pointers, promote};
use crate::sandbox_agent_token::fixture::{agent, tarball};

/// An org admin (not its owner) promotes a build whose required check failed,
/// with a reason: the plan says so before and after, production moves, the
/// checks stay as they were, and the move wrote the override's audit row.
#[tokio::test]
async fn an_org_admin_overrides_with_a_reason_and_the_move_audits_it() {
    let agent = agent().await;
    enable_gate(&agent).await;
    set_org_role(&agent, OrgRole::Admin).await;
    let slug = agent.app.slug.clone();
    let built = draft(&agent, "ovr-1", tarball(&slug, "override one")).await;
    settle(&agent, built, 1, &[("design_system", "failed")]).await;

    // The dry run previews the act: the override accepted, the checks unchanged.
    let asked = overriding(json!({ "from": "staging", "dry_run": true }), REASON);
    let (status, plan) = promote(&agent, "production", asked).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert_eq!(plan["checks"]["overridable"], true, "{plan}");
    assert_eq!(plan["checks"]["overridden"], json!({ "reason": REASON }));
    assert_eq!(plan["checks"]["unmet"], json!(["design_system"]), "{plan}");
    assert!(codes(&plan).is_empty(), "{plan}");
    assert!(audit_rows(&agent, OVERRIDDEN).await.is_empty(), "a dry run");

    let acted = overriding(json!({ "from": "staging" }), &format!("  {REASON}  "));
    let (status, plan) = promote(&agent, "production", acted).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert_eq!(plan["moved"], true, "{plan}");
    assert_eq!(plan["checks"]["overridden"], json!({ "reason": REASON }));
    assert_eq!(plan["checks"]["unmet"], json!(["design_system"]), "{plan}");
    assert_eq!(pointers(&agent).await.0, Some(built));
    let design = attempt(&agent, built, 1).await;
    assert!(
        design.contains(&("design_system".to_string(), "failed".to_string(), false)),
        "the check stays as it was: {design:?}"
    );

    let audited = audit_rows(&agent, OVERRIDDEN).await;
    assert_eq!(audited.len(), 1, "{audited:?}");
    let row = &audited[0];
    assert_eq!(row.actor_user_id, Some(agent.t.guest_id));
    assert_eq!(row.org_id, Some(agent.app.org_id));
    assert_eq!(row.workspace_id, Some(agent.app.project_id));
    assert_eq!(row.environment, "production");
    assert_eq!(row.target_type.as_deref(), Some("custom_app_environment"));
    assert_eq!(
        row.target_id,
        Some(format!("{}/production", agent.app.id)),
        "{row:?}"
    );
    assert_eq!(
        row.metadata,
        json!({
            "build": built, "reason": REASON, "unmet": ["design_system"],
            "running": [], "changes_not_seen": false,
        })
    );
    assert_eq!(audit_rows(&agent, "app.promoted").await.len(), 1);
}

/// A reason that is not one is refused, from a caller who may override.
#[tokio::test]
async fn an_override_needs_a_reason_of_ten_characters() {
    let agent = agent().await;
    let (live, _) = fixture_builds(&agent).await;
    enable_gate(&agent).await;

    for short in ["", "   hotfix  ", "123456789"] {
        let acted = overriding(json!({ "from": "staging" }), short);
        let (status, refused) = promote(&agent, "production", acted).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{short:?}: {refused}");
        assert_eq!(refused["code"], "override_reason_required", "{refused}");
        assert_eq!(refused["error"], "override_reason_required", "{refused}");
        assert!(refused["message"].is_string(), "{refused}");
    }
    let (status, refused) = make_live(&agent, Some(overriding(json!({}), "no"))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "override_reason_required", "{refused}");
    assert_eq!(pointers(&agent).await.0, Some(live), "production moved");
    assert!(audit_rows(&agent, OVERRIDDEN).await.is_empty());
}

/// A caller who may not override is refused, whatever the reason. The guest
/// as a plain member of the org still reaches the route as Oxy staff, and
/// their staff standing does not stand in for an officer's role there; a
/// publish token never overrides, whoever minted it.
#[tokio::test]
async fn an_override_needs_a_caller_who_may() {
    let agent = agent().await;
    let (live, _) = fixture_builds(&agent).await;
    enable_gate(&agent).await;

    set_org_role(&agent, OrgRole::Member).await;
    let plan = json!({ "from": "staging", "dry_run": true });
    let (status, plan) = promote(&agent, "production", plan).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert_eq!(plan["checks"]["overridable"], false, "{plan}");
    for dry_run in [true, false] {
        let acted = overriding(json!({ "from": "staging", "dry_run": dry_run }), REASON);
        let (status, refused) = promote(&agent, "production", acted).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "dry run {dry_run}: {refused}"
        );
        assert_eq!(refused["code"], "override_not_allowed", "{refused}");
        assert_eq!(refused["error"], "override_not_allowed", "{refused}");
    }
    let (status, refused) = make_live(&agent, Some(overriding(json!({}), REASON))).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert_eq!(refused["code"], "override_not_allowed", "{refused}");

    // A publish token reaches Make Live and never overrides, whoever minted it.
    set_org_role(&agent, OrgRole::Owner).await;
    let marker = Extension(AppPublishTokenAuth {
        token_id: Uuid::new_v4(),
        app_id: Some(agent.app.id),
        machine_identity: None,
    });
    let body = Bytes::from(overriding(json!({}), REASON).to_string());
    let refused = admin::publish_app(
        RefuseSandboxAgent,
        agent.t.guest(),
        Some(marker),
        Path(agent.app.id),
        body,
    )
    .await
    .expect_err("a publish token's override")
    .into_response();
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    let bytes = axum::body::to_bytes(refused.into_body(), 64 * 1024)
        .await
        .expect("a body");
    let refused: Value = serde_json::from_slice(&bytes).expect("JSON");
    assert_eq!(refused["code"], "override_not_allowed", "{refused}");

    assert_eq!(pointers(&agent).await.0, Some(live), "production moved");
    assert!(audit_rows(&agent, OVERRIDDEN).await.is_empty());
    assert!(audit_rows(&agent, "app.promoted").await.is_empty());
}

/// An override clears the three check blockers and nothing else: a build
/// whose validation did not pass is refused all the same, by the plan and by
/// the move that has none.
#[tokio::test]
async fn an_override_does_not_clear_validation_not_passed() {
    let agent = agent().await;
    let (live, staged) = fixture_builds(&agent).await;
    enable_gate(&agent).await;
    app_builds::Entity::update_many()
        .col_expr(app_builds::Column::ValidationStatus, Expr::value("failed"))
        .filter(app_builds::Column::Id.eq(staged))
        .exec(&agent.t.db)
        .await
        .expect("fail the build's validation");

    // Without the override both hold; with it, validation alone.
    let (status, refused) = promote(&agent, "production", json!({ "from": "staging" })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(
        codes(&refused),
        [
            "validation_not_passed",
            "checks_not_passed",
            "checks_running"
        ],
        "{refused}"
    );
    let acted = overriding(json!({ "from": "staging" }), REASON);
    let (status, refused) = promote(&agent, "production", acted).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["code"], "validation_not_passed", "{refused}");
    assert_eq!(codes(&refused), ["validation_not_passed"], "{refused}");

    // Make Live answers a failed validation as the bare 422 it always did.
    let (status, refused) = make_live(&agent, Some(overriding(json!({}), REASON))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(pointers(&agent).await.0, Some(live), "production moved");
    assert!(audit_rows(&agent, OVERRIDDEN).await.is_empty());
}
