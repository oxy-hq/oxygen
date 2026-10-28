//! The staff console's Factory-access routes
//! (`/admin/workspaces/{id}/factory-access[/{user_id}]`): who an operator may let
//! into a workspace's Oxygen Factory, and that a grant bounded to one org reaches
//! that org's workspaces and no other's.
//!
//! The fixture is `oxy-app`'s `admin_staff_scope` suite's, which the directories case
//! beside this one includes by path, so the two cannot drift.

use axum::extract::Path;
use axum::http::StatusCode;
use entity::org_members::OrgRole;
use entity::workspace_factory_grants;
use oxy_api_tenancy::admin::workspaces::{
    grant_factory_access, list_factory_access, revoke_factory_access,
};
use oxy_app_core::audit::RequestActor;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

// The fixture is loaded once, by the directories case; a second `mod` of the same
// file is `clippy::duplicate_mod`.
use crate::admin_staff_scope_directories::fixture::{
    Reply, World, member_of, reply, seed_user, seed_workspace, world,
};

fn actor(user: &AuthenticatedUser) -> RequestActor {
    RequestActor::session(user.clone())
}

async fn list(who: &AuthenticatedUser, ws: Uuid) -> Reply {
    reply(list_factory_access(actor(who), Path(ws)).await).await
}

async fn grant(who: &AuthenticatedUser, ws: Uuid, user: Uuid) -> Reply {
    reply(grant_factory_access(actor(who), Path((ws, user))).await).await
}

async fn revoke(who: &AuthenticatedUser, ws: Uuid, user: Uuid) -> Reply {
    reply(revoke_factory_access(actor(who), Path((ws, user))).await).await
}

async fn grants_in(w: &World, ws: Uuid) -> Vec<Uuid> {
    workspace_factory_grants::Entity::find()
        .filter(workspace_factory_grants::Column::WorkspaceId.eq(ws))
        .all(&w.db)
        .await
        .expect("read grants")
        .into_iter()
        .map(|g| g.user_id)
        .collect()
}

/// A member of org A, as the customer's own people are.
async fn member_a(w: &World, email: &str, role: OrgRole) -> AuthenticatedUser {
    let user = seed_user(&w.db, email).await;
    member_of(&w.db, user.id, w.org_a, role).await;
    user
}

#[tokio::test]
async fn an_operator_grants_and_revokes_a_member_and_the_list_says_so() {
    let w = world().await;
    let admin = member_a(&w, "admin@acme.test", OrgRole::Admin).await;
    let plain = member_a(&w, "plain@acme.test", OrgRole::Member).await;

    let before = list(&w.unbounded, w.ws_a).await;
    assert_eq!(before.status, StatusCode::OK);
    assert_eq!(before.body["enforced"], false, "the flag is off by default");
    let members = before.body["members"].as_array().unwrap();
    assert_eq!(members.len(), 2, "every member of the org can be granted");
    assert!(members.iter().all(|m| m["granted"] == false));

    assert_eq!(
        grant(&w.unbounded, w.ws_a, plain.id).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(grants_in(&w, w.ws_a).await, vec![plain.id]);

    let after = list(&w.unbounded, w.ws_a).await;
    let members = after.body["members"].as_array().unwrap();
    // Granted first, so the list reads as who has it.
    assert_eq!(members[0]["user_id"], plain.id.to_string());
    assert_eq!(members[0]["granted"], true);
    assert_eq!(members[0]["granted_by"], w.unbounded.email.clone().unwrap());
    assert_eq!(members[1]["user_id"], admin.id.to_string());
    assert_eq!(members[1]["granted"], false);
    assert_eq!(
        members[1]["org_role"], "admin",
        "an org admin has no Factory access of their own"
    );

    assert_eq!(
        revoke(&w.unbounded, w.ws_a, plain.id).await.status,
        StatusCode::NO_CONTENT
    );
    assert!(grants_in(&w, w.ws_a).await.is_empty());
}

#[tokio::test]
async fn granting_twice_is_one_grant_and_revoking_nothing_is_not_an_error() {
    let w = world().await;
    let plain = member_a(&w, "plain@acme.test", OrgRole::Member).await;

    assert_eq!(
        grant(&w.unbounded, w.ws_a, plain.id).await.status,
        StatusCode::NO_CONTENT
    );
    // The second operator does not take the grant over: the first author stands.
    assert_eq!(
        grant(&w.owner, w.ws_a, plain.id).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(grants_in(&w, w.ws_a).await, vec![plain.id]);
    let row = workspace_factory_grants::Entity::find_by_id((w.ws_a, plain.id))
        .one(&w.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.granted_by, Some(w.unbounded.id));

    assert_eq!(
        revoke(&w.unbounded, w.ws_a, plain.id).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        revoke(&w.unbounded, w.ws_a, plain.id).await.status,
        StatusCode::NO_CONTENT
    );
}

/// A grant opens nothing for someone outside the org, so none is written for them.
#[tokio::test]
async fn only_a_member_of_the_workspaces_org_can_be_granted() {
    let w = world().await;
    let stranger = seed_user(&w.db, "stranger@elsewhere.test").await;
    member_of(&w.db, stranger.id, w.org_b, OrgRole::Owner).await;

    let r = grant(&w.unbounded, w.ws_a, stranger.id).await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(r.body["error"], "not_an_org_member");
    assert!(grants_in(&w, w.ws_a).await.is_empty());

    assert_eq!(
        grant(&w.unbounded, w.ws_a, Uuid::new_v4()).await.status,
        StatusCode::NOT_FOUND,
        "no such user"
    );
}

/// A grant whose holder has left the org is listed (so it can be removed), is not
/// offered as a member, and goes when revoked.
#[tokio::test]
async fn a_grant_whose_holder_left_the_org_is_listed_and_removable() {
    let w = world().await;
    let leaver = member_a(&w, "leaver@acme.test", OrgRole::Member).await;
    grant(&w.unbounded, w.ws_a, leaver.id).await;

    entity::org_members::Entity::delete_many()
        .filter(entity::org_members::Column::UserId.eq(leaver.id))
        .exec(&w.db)
        .await
        .unwrap();

    let listed = list(&w.unbounded, w.ws_a).await;
    let members = listed.body["members"].as_array().unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0]["granted"], true);
    assert!(members[0]["org_role"].is_null());

    assert_eq!(
        revoke(&w.unbounded, w.ws_a, leaver.id).await.status,
        StatusCode::NO_CONTENT
    );
    assert!(grants_in(&w, w.ws_a).await.is_empty());
}

/// A grant grants Factory access on the one workspace and no other.
#[tokio::test]
async fn a_grant_is_for_one_workspace() {
    let w = world().await;
    let second_ws = seed_workspace(&w.db, "a2", Some(w.org_a)).await;
    let plain = member_a(&w, "plain@acme.test", OrgRole::Member).await;

    grant(&w.unbounded, w.ws_a, plain.id).await;
    assert_eq!(grants_in(&w, w.ws_a).await, vec![plain.id]);
    assert!(grants_in(&w, second_ws).await.is_empty());
}

/// A grant bounded to org A reaches org A's workspaces; org B's and the org-less
/// workspace answer the 404 a missing row gets — on the list, the grant and the
/// revoke alike — and a Global Owner reaches all of them.
#[tokio::test]
async fn a_bounded_grant_reaches_only_its_own_orgs_workspaces() {
    let w = world().await;
    let plain_a = member_a(&w, "plain@acme.test", OrgRole::Member).await;
    let plain_b = seed_user(&w.db, "plain@other.test").await;
    member_of(&w.db, plain_b.id, w.org_b, OrgRole::Member).await;

    assert_eq!(list(&w.bounded, w.ws_a).await.status, StatusCode::OK);
    assert_eq!(
        grant(&w.bounded, w.ws_a, plain_a.id).await.status,
        StatusCode::NO_CONTENT
    );

    for (what, ws) in [("org B's", w.ws_b), ("the org-less", w.ws_orphan)] {
        assert_eq!(
            list(&w.bounded, ws).await.status,
            StatusCode::NOT_FOUND,
            "listing {what} workspace"
        );
        assert_eq!(
            grant(&w.bounded, ws, plain_b.id).await.status,
            StatusCode::NOT_FOUND,
            "granting in {what} workspace"
        );
        assert_eq!(
            revoke(&w.bounded, ws, plain_b.id).await.status,
            StatusCode::NOT_FOUND,
            "revoking in {what} workspace"
        );
    }
    assert!(grants_in(&w, w.ws_b).await.is_empty());

    assert_eq!(
        grant(&w.owner, w.ws_b, plain_b.id).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(grants_in(&w, w.ws_b).await, vec![plain_b.id]);
}

/// An org-less workspace has no members to name: the same 404 for everyone.
#[tokio::test]
async fn a_workspace_with_no_org_has_nobody_to_grant() {
    let w = world().await;
    assert_eq!(
        list(&w.owner, w.ws_orphan).await.status,
        StatusCode::NOT_FOUND
    );
}
