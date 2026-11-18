//! `GET /admin/users/{id}` and `GET /admin/users/{id}/org-memberships` — a person,
//! as a bounded grant sees them.
//!
//! A user belongs to no single org, so the fence is their memberships: a bounded grant
//! sees a user who is a member of at least one org it reaches, and of that user only
//! the rows those orgs own — memberships, workspace memberships, invitations and the
//! partners they operate. A user in no org is platform-level. Anyone else answers what
//! a missing user answers.

use axum::extract::Path;
use axum::http::StatusCode;
use chrono::{Duration, Utc};
use entity::org_invitations::{self, InviteStatus};
use entity::org_members::OrgRole;
use entity::workspace_members::{self, WorkspaceRole};
use entity::{partner_grants, partner_role_bindings};
use oxy_api_identity::admin::users_admin::{get_user_detail, list_org_memberships};
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection};
use uuid::Uuid;

use super::fixture::{Reply, World, as_actor, member_of, reply, seed_user, world};

async fn detail_as(actor: &AuthenticatedUser, user: Uuid) -> Reply {
    reply(get_user_detail(as_actor(actor), Path(user)).await).await
}

async fn memberships_as(actor: &AuthenticatedUser, user: Uuid) -> Reply {
    reply(list_org_memberships(as_actor(actor), Path(user)).await).await
}

async fn workspace_member_of(db: &DatabaseConnection, user: Uuid, workspace: Uuid) {
    workspace_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        workspace_id: ActiveValue::Set(workspace),
        user_id: ActiveValue::Set(user),
        role: ActiveValue::Set(WorkspaceRole::Member),
        created_at: ActiveValue::Set(Utc::now().fixed_offset()),
        updated_at: ActiveValue::Set(Utc::now().fixed_offset()),
    }
    .insert(db)
    .await
    .expect("seed workspace membership");
}

pub(super) async fn invite(w: &World, org: Uuid, invitee: &AuthenticatedUser) {
    let id = Uuid::new_v4();
    org_invitations::ActiveModel {
        id: ActiveValue::Set(id),
        org_id: ActiveValue::Set(org),
        email: ActiveValue::Set(invitee.email.clone().unwrap_or_default()),
        role: ActiveValue::Set(OrgRole::Admin),
        invited_by: ActiveValue::Set(w.unbounded.id),
        token: ActiveValue::Set(format!("tok-{id}")),
        status: ActiveValue::Set(InviteStatus::Pending),
        expires_at: ActiveValue::Set((Utc::now() + Duration::days(7)).fixed_offset()),
        created_at: ActiveValue::NotSet,
    }
    .insert(&w.db)
    .await
    .expect("seed invitation");
}

/// Make `org` a partner and `membership` one of its operators.
pub(super) async fn operate_partner(db: &DatabaseConnection, org: Uuid, membership: Uuid) {
    partner_grants::ActiveModel {
        org_id: ActiveValue::Set(org),
        status: ActiveValue::Set("active".to_string()),
        created_by: ActiveValue::Set(None),
        created_at: ActiveValue::Set(Utc::now().fixed_offset()),
    }
    .insert(db)
    .await
    .expect("seed partner grant");
    partner_role_bindings::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_member_id: ActiveValue::Set(membership),
        created_at: ActiveValue::Set(Utc::now().fixed_offset()),
    }
    .insert(db)
    .await
    .expect("seed partner role binding");
}

#[tokio::test]
async fn a_bounded_grant_cannot_open_a_user_outside_its_orgs() {
    let w = world().await;
    let in_b = seed_user(&w.db, "in-b@tenant.test").await;
    member_of(&w.db, in_b.id, w.org_b, OrgRole::Owner).await;
    let in_none = seed_user(&w.db, "in-none@tenant.test").await;

    let missing = detail_as(&w.bounded, Uuid::new_v4()).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    for (who, target) in [("a member of org B", &in_b), ("a user in no org", &in_none)] {
        let refused = detail_as(&w.bounded, target.id).await;
        assert_eq!(
            refused.status,
            StatusCode::NOT_FOUND,
            "{who}: {}",
            refused.body
        );
        assert_eq!(refused.body, missing.body, "{who}");
        let refused = memberships_as(&w.bounded, target.id).await;
        assert_eq!(
            refused.status,
            StatusCode::NOT_FOUND,
            "{who}: {}",
            refused.body
        );
        assert_eq!(refused.body, missing.body, "{who}");
    }

    for (who, actor) in w.everything_readers() {
        for target in [&in_b, &in_none] {
            assert_eq!(
                detail_as(actor, target.id).await.status,
                StatusCode::OK,
                "{who}"
            );
            assert_eq!(
                memberships_as(actor, target.id).await.status,
                StatusCode::OK,
                "{who}"
            );
        }
    }
}

#[tokio::test]
async fn a_bounded_grant_sees_only_its_own_orgs_rows_of_a_shared_user() {
    let w = world().await;
    let shared = seed_user(&w.db, "shared@tenant.test").await;
    member_of(&w.db, shared.id, w.org_a, OrgRole::Member).await;
    let in_b = member_of(&w.db, shared.id, w.org_b, OrgRole::Owner).await;
    operate_partner(&w.db, w.org_b, in_b).await;
    for ws in [w.ws_a, w.ws_b, w.ws_orphan] {
        workspace_member_of(&w.db, shared.id, ws).await;
    }
    invite(&w, w.org_a, &shared).await;
    invite(&w, w.org_b, &shared).await;

    let seen = detail_as(&w.bounded, shared.id).await;
    assert_eq!(seen.status, StatusCode::OK, "{}", seen.body);
    let a = w.org_a.to_string();
    assert_eq!(
        seen.column(Some("org_memberships"), "org_id"),
        vec![a.clone()]
    );
    assert_eq!(
        seen.column(Some("workspace_memberships"), "workspace_id"),
        vec![w.ws_a.to_string()],
        "org B's and the org-less workspace are listed to a grant bounded to org A"
    );
    assert_eq!(seen.column(Some("invitations"), "org_id"), vec![a.clone()]);
    assert_eq!(
        seen.column(Some("partners"), "id"),
        Vec::<String>::new(),
        "org B, by name, as a partner the user operates"
    );
    let listed = memberships_as(&w.bounded, shared.id).await;
    assert_eq!(listed.status, StatusCode::OK);
    assert_eq!(listed.column(None, "org_id"), vec![a]);

    for (who, actor) in w.everything_readers() {
        let seen = detail_as(actor, shared.id).await;
        assert_eq!(
            seen.column(Some("org_memberships"), "org_id").len(),
            2,
            "{who}"
        );
        assert_eq!(
            seen.column(Some("workspace_memberships"), "workspace_id")
                .len(),
            3,
            "{who}"
        );
        assert_eq!(seen.column(Some("invitations"), "org_id").len(), 2, "{who}");
        assert_eq!(
            seen.column(Some("partners"), "id"),
            vec![w.org_b.to_string()],
            "{who}"
        );
        assert_eq!(
            memberships_as(actor, shared.id)
                .await
                .column(None, "org_id")
                .len(),
            2,
            "{who}"
        );
    }
}
