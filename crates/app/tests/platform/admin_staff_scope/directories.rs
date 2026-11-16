//! The directories and rollups: the grant's scope itself, and the one org-owned
//! write on the users router. (`/admin/metrics/llm-usage` moved with its
//! handlers to `oxy-api-observability`'s `admin_staff_scope_llm_usage`, admin
//! split PR 1.) (`/admin/workspace-health` moved with its handlers
//! to `oxy-api-workspace-health`'s `admin_staff_scope`.) None was in the review; the audit of the rest of the
//! console found them. (`/admin/orgs-meta` and `/admin/workspaces-meta` moved
//! with their handlers to `oxy-api-tenancy`'s `admin_staff_scope_directories`.)

use axum::extract::{OriginalUri, Path};
use axum::http::{StatusCode, Uri};
use chrono::{Duration, Utc};
use entity::org_invitations::{self, InviteStatus};
use entity::org_members::OrgRole;
use oxy_app::server::api::admin::scope::list_scope;
use oxy_app::server::api::admin::users_admin::revoke_user_invitation;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait};
use uuid::Uuid;

use super::fixture::{World, as_actor, grant, seed_user, world};

fn uri(path: &'static str) -> OriginalUri {
    OriginalUri(Uri::from_static(path))
}

/// The fact every listing is narrowed by, read through the loader and the model.
#[tokio::test]
async fn list_scope_is_the_grants_orgs_and_nothing_for_no_standing() {
    let w = world().await;
    assert_eq!(list_scope(&w.db, &w.bounded).await, Ok(Some(vec![w.org_a])));
    assert_eq!(list_scope(&w.db, &w.unbounded).await, Ok(None));
    assert_eq!(list_scope(&w.db, &w.owner).await, Ok(None));

    // A grant bounded to nothing reaches nothing — never "unbounded by omission".
    let empty = seed_user(&w.db, "empty@staff-scope.test").await;
    grant(&w.db, "empty@staff-scope.test", Some(&[])).await;
    assert_eq!(list_scope(&w.db, &empty).await, Ok(Some(Vec::new())));

    // No standing at all: unreachable behind `platform_cap_guard`, and if that ever
    // changes the listing comes back empty rather than whole.
    let stranger = seed_user(&w.db, "stranger@tenant.test").await;
    assert_eq!(list_scope(&w.db, &stranger).await, Ok(Some(Vec::new())));
}

async fn invitation(w: &World, org: Uuid, invitee: &AuthenticatedUser) -> Uuid {
    let id = Uuid::new_v4();
    org_invitations::ActiveModel {
        id: ActiveValue::Set(id),
        org_id: ActiveValue::Set(org),
        email: ActiveValue::Set(invitee.email.clone().unwrap_or_default()),
        role: ActiveValue::Set(OrgRole::Member),
        invited_by: ActiveValue::Set(w.unbounded.id),
        token: ActiveValue::Set(format!("tok-{id}")),
        status: ActiveValue::Set(InviteStatus::Pending),
        expires_at: ActiveValue::Set((Utc::now() + Duration::days(7)).fixed_offset()),
        created_at: ActiveValue::NotSet,
    }
    .insert(&w.db)
    .await
    .expect("seed invitation");
    id
}

/// An invitation belongs to the org that issued it; the route names only the
/// invitee, so this was a cross-tenant write by id.
#[tokio::test]
async fn a_bounded_grant_cannot_revoke_another_orgs_invitation() {
    let w = world().await;
    let invitee = seed_user(&w.db, "invitee@tenant.test").await;
    let theirs = invitation(&w, w.org_b, &invitee).await;
    let mine = invitation(&w, w.org_a, &invitee).await;

    let refused = revoke_user_invitation(as_actor(&w.bounded), Path((invitee.id, theirs))).await;
    assert_eq!(refused, Err(StatusCode::NOT_FOUND));
    assert!(
        org_invitations::Entity::find_by_id(theirs)
            .one(&w.db)
            .await
            .expect("read invitation")
            .is_some(),
        "org B's invitation was revoked by a grant bounded to org A"
    );

    let revoked = revoke_user_invitation(as_actor(&w.bounded), Path((invitee.id, mine))).await;
    assert_eq!(revoked, Ok(StatusCode::NO_CONTENT));
    let revoked = revoke_user_invitation(as_actor(&w.owner), Path((invitee.id, theirs))).await;
    assert_eq!(revoked, Ok(StatusCode::NO_CONTENT));
}
