//! The standing an **agent token** carries (API-tokens design, "The agent
//! token (2026-10-07)"): its owner's staff and partner standing, only where
//! the approval said so **and** the owner holds it when the code is redeemed.
//! Asking for a standing one does not hold is not an error.

use axum::http::StatusCode;
use entity::org_members::{self, OrgRole};
use entity::{
    app_admins, partner_capabilities, partner_grants, partner_orgs, partner_role_bindings,
};
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter};
use serde_json::json;
use uuid::Uuid;

use super::agent_token::{VERIFIER, agent_mint, authorize, exchange, id_of, minted, secret_of};
use super::sandbox_agent::post_as;
use super::stack::{get_as, get_in_session, ids, make_staff};
use super::{Fixture, audit_rows, fixture, seed_org};

#[tokio::test]
async fn a_standing_asked_for_and_held_is_carried() {
    let fx = fixture().await;
    make_staff(&fx.db, fx.user.email.as_deref().unwrap()).await;
    let minted = minted(&fx.cookie, "ops-laptop", json!({ "standing": true })).await;
    assert_eq!(minted["token"]["platform"], true);
    assert_eq!(
        minted["token"]["partner"], false,
        "only the standing its owner holds"
    );
    assert_eq!(minted["token"]["all_access"], true);
}

#[tokio::test]
async fn a_standing_asked_for_and_not_held_is_no_error_and_no_standing() {
    let fx = fixture().await;
    let (status, approved) = authorize(
        &fx.cookie,
        "laptop",
        Some(agent_mint(json!({ "standing": true }))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "asking is not an error: {approved}");
    let (status, minted) = exchange(approved["code"].as_str().unwrap(), VERIFIER).await;
    assert_eq!(status, StatusCode::OK, "{minted}");
    assert_eq!(minted["token"]["platform"], false);
    assert_eq!(minted["token"]["partner"], false);
}

#[tokio::test]
async fn a_standing_held_and_not_asked_for_is_not_carried() {
    let fx = fixture().await;
    make_staff(&fx.db, fx.user.email.as_deref().unwrap()).await;
    for extra in [
        json!({}),
        json!({ "standing": false }),
        json!({ "standing": null }),
    ] {
        let minted = minted(&fx.cookie, "ops-laptop", extra.clone()).await;
        assert_eq!(minted["token"]["platform"], false, "{extra}");
        assert_eq!(minted["token"]["partner"], false, "{extra}");
    }
}

/// Make the fixture's user a partner operator: a member, holding partner
/// access, of an org with an active partner grant that manages one client.
async fn make_partner(fx: &Fixture) {
    let (partner_org, client) = (seed_org(&fx.db).await, seed_org(&fx.db).await);
    let membership = org_members::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(partner_org),
        user_id: Set(fx.user.id),
        role: Set(OrgRole::Member),
        ..Default::default()
    }
    .insert(&fx.db)
    .await
    .expect("seed the partner org membership");
    partner_grants::ActiveModel {
        org_id: Set(partner_org),
        status: Set("active".into()),
        created_by: Set(None),
        ..Default::default()
    }
    .insert(&fx.db)
    .await
    .expect("seed the partner grant");
    partner_capabilities::ActiveModel {
        org_id: Set(partner_org),
        manage_members: Set(true),
        manage_apps: Set(false),
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
        partner_org_id: Set(partner_org),
        managed_org_id: Set(client),
        created_by: Set(None),
        ..Default::default()
    }
    .insert(&fx.db)
    .await
    .expect("seed the managed client");
}

#[tokio::test]
async fn a_partner_carries_the_partner_standing_and_not_the_staff_one() {
    let fx = fixture().await;
    make_partner(&fx).await;
    let minted = minted(&fx.cookie, "partner-laptop", json!({ "standing": true })).await;
    assert_eq!(minted["token"]["partner"], true);
    assert_eq!(minted["token"]["platform"], false);
}

#[tokio::test]
async fn the_standing_carried_is_the_one_held_when_the_code_is_redeemed() {
    let fx = fixture().await;
    let email = fx.user.email.clone().unwrap();
    make_staff(&fx.db, &email).await;
    let (status, approved) = authorize(
        &fx.cookie,
        "ops-laptop",
        Some(agent_mint(json!({ "standing": true }))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{approved}");

    // Between the approval and the exchange the grant is taken away, as the
    // admin console takes one away: the row, then the 60-second grant cache.
    app_admins::Entity::delete_many()
        .filter(app_admins::Column::Email.eq(email))
        .exec(&fx.db)
        .await
        .expect("take the grant away");
    oxy_app::server::authz::globals::invalidate_admin_cache();

    let (status, minted) = exchange(approved["code"].as_str().unwrap(), VERIFIER).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "still a token, with no standing: {minted}"
    );
    assert_eq!(minted["token"]["platform"], false);
}

/// The staff list of standing tokens (`/api/admin/standing-tokens`) was not
/// changed for the agent token: one that carries a standing is on it because
/// it is a personal token that carries one, and staff end it there.
#[tokio::test]
async fn one_that_carries_a_standing_is_on_the_staff_list_and_staff_can_end_it() {
    let fx = fixture().await;
    make_staff(&fx.db, fx.user.email.as_deref().unwrap()).await;
    let carries = minted(&fx.cookie, "ops-laptop", json!({ "standing": true })).await;
    let plain = minted(&fx.cookie, "ops-laptop", json!({})).await;

    let (status, list) = get_in_session(&fx, "/admin/standing-tokens").await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(
        ids(&list["tokens"]),
        vec![id_of(&carries)],
        "the one that carries a standing, and not the one that carries none: {list}"
    );
    assert_eq!(list["tokens"][0]["source"], "oxyc_agent");
    assert_eq!(list["tokens"][0]["name"], "agent on ops-laptop");

    let revoke = format!("/admin/standing-tokens/{}/revoke", id_of(&carries));
    let (status, revoked) = post_as(&fx.cookie, &revoke, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{revoked}");
    let (status, _) = get_as(&secret_of(&carries), "/auth/token").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = get_as(&secret_of(&plain), "/auth/token").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the other agent's token is untouched"
    );
    let rows = audit_rows(&fx.db, "token.revoked").await;
    assert_eq!(rows.len(), 1, "one token, which reaches no org: one row");
    assert_eq!(rows[0].metadata["reason"], "staff");
    assert_eq!(rows[0].metadata["source"], "oxyc_agent");
}
