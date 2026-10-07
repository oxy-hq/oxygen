//! The partner console's promote
//! (`POST /partners/{partner}/apps/{app}/publish`) refuses a draft a sandbox
//! agent token published **with the body every other promote answers**
//! (`custom_apps_agent_built`): `409 draft_published_by_agent`, naming the
//! build, where it was published, the token and its minter. The handler used
//! to cut the refusal down to its status, so a partner got a bare `409` and
//! no reason.
//!
//! A person's draft promotes through it as it always did, and an app outside
//! the partner's clients is still the bare `404` it always was.
//!
//! The draft is seeded as a publish leaves one — the build's row, staging's
//! pointer and the event that records it. The publish itself is
//! `staging_draft`'s and `staging_promote`'s, in `oxy-app`.

use axum::http::StatusCode;
use entity::org_members::{self, OrgRole};
use entity::{
    app_builds, apps, partner_capabilities, partner_grants, partner_orgs, partner_role_bindings,
};
use oxy_app::server::api::custom_apps_environments::{EnvAction, record_move};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait, IntoActiveModel};
use serde_json::{Value, json};
use uuid::Uuid;

use super::sandbox_agent::{another_session, minted, post_as, staff_with_app};
use super::stack::published_app;
use super::{Fixture, audit_rows, seed_org, seed_workspace_in};

const REFUSED: &str = "draft_published_by_agent";

/// An org with an active partner grant whose ceiling is `manage_apps` alone.
async fn partner_org(fx: &Fixture) -> Uuid {
    let org = seed_org(&fx.db).await;
    partner_grants::ActiveModel {
        org_id: Set(org),
        status: Set("active".into()),
        created_by: Set(None),
        ..Default::default()
    }
    .insert(&fx.db)
    .await
    .expect("seed the partner grant");
    partner_capabilities::ActiveModel {
        org_id: Set(org),
        manage_members: Set(false),
        manage_apps: Set(true),
        develop_apps: Set(false),
        view_audit: Set(false),
        manage_billing: Set(false),
        manage_secrets: Set(false),
        create_orgs: Set(false),
        manage_org_settings: Set(false),
        ..Default::default()
    }
    .insert(&fx.db)
    .await
    .expect("seed the partner ceiling");
    org
}

/// A partner that manages `client`, and one of its operators: the partner
/// org's id, and the operator's browser session.
async fn operator_managing(fx: &Fixture, client: Uuid) -> (Uuid, String) {
    let partner = partner_org(fx).await;
    let (operator, cookie) = another_session(&fx.db, "partner").await;
    let membership = org_members::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(partner),
        user_id: Set(operator.id),
        role: Set(OrgRole::Member),
        ..Default::default()
    }
    .insert(&fx.db)
    .await
    .expect("seed the partner org membership");
    partner_role_bindings::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_member_id: Set(membership.id),
        ..Default::default()
    }
    .insert(&fx.db)
    .await
    .expect("seed partner access");
    partner_orgs::ActiveModel {
        id: Set(Uuid::new_v4()),
        partner_org_id: Set(partner),
        managed_org_id: Set(client),
        created_by: Set(None),
        ..Default::default()
    }
    .insert(&fx.db)
    .await
    .expect("seed the managed client");
    (partner, cookie)
}

/// A draft of `app` as a publish leaves one: the build's row, staging's
/// pointer and its event, and the app's draft pointer. `token` is the sandbox
/// agent token that published it, when one did.
async fn draft(fx: &Fixture, app: &apps::Model, label: &str, token: Option<Uuid>) -> Uuid {
    let id = Uuid::new_v4();
    app_builds::ActiveModel {
        id: Set(id),
        app_id: Set(app.id),
        build_id: Set(label.to_string()),
        s3_prefix: Set(format!("customer-apps/{}/builds/{label}/", app.id)),
        created_at: Set(chrono::Utc::now().fixed_offset()),
        published_by: Set(Some(fx.user.id)),
        validation_status: Set("passed".into()),
        published_token_id: Set(token),
        ..Default::default()
    }
    .insert(&fx.db)
    .await
    .expect("seed the build");
    let staging = AppEnvironment::Staging;
    record_move(
        &fx.db,
        app.id,
        &staging,
        Some(id),
        EnvAction::Publish,
        Some(fx.user.id),
    )
    .await
    .expect("move staging's pointer");
    let mut row = app.clone().into_active_model();
    row.draft_build_id = Set(Some(id));
    row.update(&fx.db).await.expect("move the draft pointer");
    id
}

async fn app_row(fx: &Fixture, id: Uuid) -> apps::Model {
    apps::Entity::find_by_id(id)
        .one(&fx.db)
        .await
        .expect("read the app")
        .expect("the app")
}

#[tokio::test]
async fn the_partner_promote_answers_an_agents_draft_with_the_coded_refusal() {
    let (fx, app) = staff_with_app().await;
    let (token, _secret) = minted(&fx, &[app.id]).await;
    let (partner, cookie) = operator_managing(&fx, fx.org_id).await;
    let promote = format!("/partners/{partner}/apps/{}/publish", app.id);

    let unapproved = draft(&fx, &app, "agent-draft-1", Some(token)).await;
    let (status, body) = post_as(&cookie, &promote, json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], REFUSED, "{body}");
    assert_eq!(body["error"], REFUSED, "{body}");
    assert_eq!(body["build_id"], "agent-draft-1", "{body}");
    assert_eq!(body["environment"], "staging", "{body}");
    assert_eq!(body["token_id"], json!(token), "{body}");
    assert_eq!(body["token_name"], "agent", "{body}");
    assert_eq!(body["minter"], json!(fx.user.email), "{body}");
    let message = body["message"].as_str().unwrap_or_default();
    for said in [
        "agent-draft-1",
        "published to staging",
        "under your own name",
    ] {
        assert!(message.contains(said), "{said} in {message}");
    }

    // Nothing moved, and nothing was recorded as published.
    let row = app_row(&fx, app.id).await;
    assert_eq!(row.published_build_id, None, "production never moved");
    assert_eq!(row.draft_build_id, Some(unapproved));
    assert!(row.last_promoted_by.is_none());
    let published = audit_rows(&fx.db, "partner.app.published").await;
    assert!(published.is_empty(), "{published:?}");
}

#[tokio::test]
async fn the_partner_promote_ships_a_persons_draft_and_keeps_its_other_answers() {
    let (fx, app) = staff_with_app().await;
    let (partner, cookie) = operator_managing(&fx, fx.org_id).await;
    let promote = format!("/partners/{partner}/apps/{}/publish", app.id);

    let theirs = draft(&fx, &app, "person-draft-1", None).await;
    let (status, body) = post_as(&cookie, &promote, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], json!(app.id), "{body}");
    assert_eq!(body["published"], true, "{body}");
    assert!(body.get("code").is_none(), "{body}");
    assert_eq!(
        app_row(&fx, app.id).await.published_build_id,
        Some(theirs),
        "production serves the person's draft"
    );
    assert_eq!(audit_rows(&fx.db, "partner.app.published").await.len(), 1);

    // An app of an org the partner does not manage: the bare status, no body.
    let other_org = seed_org(&fx.db).await;
    let other_workspace = seed_workspace_in(&fx.db, other_org).await;
    let elsewhere = published_app(&fx.db, other_org, other_workspace).await;
    let uri = format!("/partners/{partner}/apps/{}/publish", elsewhere.id);
    let (status, body) = post_as(&cookie, &uri, json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body, Value::Null, "a bare status, as before");
}
