//! `POST /admin/users/{id}/org-memberships` — putting a person into an org, as a bounded
//! grant may.
//!
//! The org in the body is fenced like any by-id write. The **person** needs a fence too.
//! A user in no org is platform-level, outside every bounded grant, and
//! [`super::user_status`] refuses to suspend one for that reason. Without a fence on the
//! target, a grant bounded to org A could add such a user to A and then suspend them:
//! two calls that together reach a person the rule says it cannot. So a bounded grant
//! adds only someone it already reaches (a member of one of its orgs). Anyone else gets
//! the answer a missing user gets, a pending invitee included: the grant can write that
//! invitation itself, through an assume session into one of its orgs.
//!
//! A service account is never an org member (`oxy_auth::token::service_account`), for
//! any caller.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use chrono::Utc;
use entity::org_members::{self, OrgRole};
use entity::users::{self, UserStatus};
use entity::{org_frontline_members, service_accounts};
use oxy_api_identity::admin::users_admin::{
    AddToOrgBody, SetStatusBody, add_to_org, set_user_status,
};
use oxy_app_core::audit::RequestActor;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use uuid::Uuid;

use super::fixture::{Reply, grant, member_of, reply, seed_org, seed_user, world};
use super::user_detail::invite;

async fn add(actor: &AuthenticatedUser, user: Uuid, org: Uuid) -> Reply {
    let body = Json(AddToOrgBody {
        org_id: org,
        role: "member".to_string(),
    });
    let actor = RequestActor::session(actor.clone());
    reply(add_to_org(actor, Path(user), body).await).await
}

async fn suspend(actor: &AuthenticatedUser, user: Uuid) -> Reply {
    let body = Json(SetStatusBody {
        status: "deleted".to_string(),
    });
    let actor = RequestActor::session(actor.clone());
    reply(set_user_status(actor, Path(user), body).await).await
}

async fn orgs_of(db: &DatabaseConnection, user: Uuid) -> Vec<Uuid> {
    org_members::Entity::find()
        .filter(org_members::Column::UserId.eq(user))
        .all(db)
        .await
        .expect("read memberships")
        .into_iter()
        .map(|m| m.org_id)
        .collect()
}

async fn status_of(db: &DatabaseConnection, user: Uuid) -> UserStatus {
    users::Entity::find_by_id(user)
        .one(db)
        .await
        .expect("read user")
        .expect("user exists")
        .status
}

/// A `users` row with no address, the shape a service account and a frontline worker
/// share.
async fn seed_addressless(db: &DatabaseConnection, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(None),
        name: ActiveValue::Set(name.to_string()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(false),
        magic_link_token: ActiveValue::Set(None),
        magic_link_token_expires_at: ActiveValue::Set(None),
        status: ActiveValue::Set(UserStatus::Active),
        created_at: ActiveValue::NotSet,
        last_login_at: ActiveValue::NotSet,
    }
    .insert(db)
    .await
    .expect("seed addressless user");
    id
}

/// An org-owned service account: a `users` row with no address, and a
/// `service_accounts` row naming its org. No `org_members` row.
async fn seed_service_account(db: &DatabaseConnection, org: Uuid) -> Uuid {
    let id = seed_addressless(db, "ci-bot").await;
    service_accounts::ActiveModel {
        user_id: ActiveValue::Set(id),
        org_id: ActiveValue::Set(org),
        org_role: ActiveValue::Set(service_accounts::ROLE_MEMBER.to_string()),
        name: ActiveValue::Set(format!("ci-bot-{}", id.simple())),
        description: ActiveValue::Set(None),
        created_by: ActiveValue::Set(None),
        created_at: ActiveValue::NotSet,
        disabled_at: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed service account");
    id
}

/// A frontline worker enrolled in `org`: no address, no `org_members` row.
async fn seed_frontline_worker(db: &DatabaseConnection, org: Uuid) -> Uuid {
    let id = seed_addressless(db, "crew").await;
    org_frontline_members::ActiveModel {
        org_id: ActiveValue::Set(org),
        user_id: ActiveValue::Set(id),
        status: ActiveValue::Set(org_frontline_members::STATUS_ACTIVE.to_string()),
        created_at: ActiveValue::Set(Utc::now().fixed_offset()),
    }
    .insert(db)
    .await
    .expect("seed frontline worker");
    id
}

/// The finding: add a user in no org to org A, then suspend them. Each call alone looked
/// fenced; together they reached a person no bounded grant reaches.
#[tokio::test]
async fn a_bounded_grant_cannot_add_a_user_in_no_org_and_then_suspend_them() {
    let w = world().await;
    let loner = seed_user(&w.db, "loner@tenant.test").await;

    let added = add(&w.bounded, loner.id, w.org_a).await;
    let suspended = suspend(&w.bounded, loner.id).await;
    assert_eq!(
        status_of(&w.db, loner.id).await,
        UserStatus::Active,
        "a grant bounded to org A added a user in no org to A ({}), then suspended them ({})",
        added.status,
        suspended.status
    );
    let missing = add(&w.bounded, Uuid::new_v4(), w.org_a).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(added.status, StatusCode::NOT_FOUND, "{}", added.body);
    assert_eq!(
        added.body, missing.body,
        "a refusal must read as a missing user"
    );
    assert_eq!(suspended.status, StatusCode::NOT_FOUND);
    assert!(orgs_of(&w.db, loner.id).await.is_empty());

    // A member of org B only is no closer: they are not the grant's to see either.
    let in_b = seed_user(&w.db, "in-b@tenant.test").await;
    member_of(&w.db, in_b.id, w.org_b, OrgRole::Member).await;
    let added = add(&w.bounded, in_b.id, w.org_a).await;
    assert_eq!(added.status, StatusCode::NOT_FOUND, "{}", added.body);
    assert_eq!(orgs_of(&w.db, in_b.id).await, vec![w.org_b]);

    // An all-orgs grant and the Global Owner keep what they had.
    for (n, (who, actor)) in w.everything_readers().into_iter().enumerate() {
        let target = seed_user(&w.db, &format!("loner-{n}@tenant.test")).await;
        let added = add(actor, target.id, w.org_a).await;
        assert_eq!(added.status, StatusCode::CREATED, "{who}: {}", added.body);
        assert_eq!(orgs_of(&w.db, target.id).await, vec![w.org_a], "{who}");
    }
}

#[tokio::test]
async fn a_bounded_grant_adds_only_someone_it_already_reaches() {
    let w = world().await;
    let org_c = seed_org(&w.db, "c").await;
    let over_a_and_c = seed_user(&w.db, "bounded-ac@staff-scope.test").await;
    grant(
        &w.db,
        "bounded-ac@staff-scope.test",
        Some(&[w.org_a, org_c]),
    )
    .await;

    // Already a member of org A: the grant reaches them, so its other org is open.
    let in_a = seed_user(&w.db, "in-a@tenant.test").await;
    member_of(&w.db, in_a.id, w.org_a, OrgRole::Member).await;
    let added = add(&over_a_and_c, in_a.id, org_c).await;
    assert_eq!(added.status, StatusCode::CREATED, "{}", added.body);
    let mut orgs = orgs_of(&w.db, in_a.id).await;
    orgs.sort();
    let mut want = vec![w.org_a, org_c];
    want.sort();
    assert_eq!(orgs, want);
    // Adding them to the org they are in is the membership's own conflict.
    let again = add(&w.bounded, in_a.id, w.org_a).await;
    assert_eq!(again.status, StatusCode::CONFLICT, "{}", again.body);

    // In no org yet, with a pending invitation from org A. Not reach: an assume session
    // into A is A's Owner, so the grant could have written that invitation itself. The
    // invitee joins by accepting it.
    for org in [w.org_a, w.org_b] {
        let invited = seed_user(&w.db, &format!("invited-{org}@tenant.test")).await;
        invite(&w, org, &invited).await;
        let added = add(&w.bounded, invited.id, w.org_a).await;
        assert_eq!(added.status, StatusCode::NOT_FOUND, "{}", added.body);
        assert!(orgs_of(&w.db, invited.id).await.is_empty());
    }
    // An all-orgs grant adds an invitee, as before.
    let invited = seed_user(&w.db, "invited-unbounded@tenant.test").await;
    invite(&w, w.org_a, &invited).await;
    let added = add(&w.unbounded, invited.id, w.org_a).await;
    assert_eq!(added.status, StatusCode::CREATED, "{}", added.body);
}

#[tokio::test]
async fn a_service_account_is_never_made_an_org_member() {
    let w = world().await;
    let account = seed_service_account(&w.db, w.org_a).await;

    // A bounded grant cannot see it (it is in no org by membership), so it is missing.
    let refused = add(&w.bounded, account, w.org_a).await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND, "{}", refused.body);

    // Unbounded callers can see it, so they are told why.
    for (who, actor) in w.everything_readers() {
        let refused = add(actor, account, w.org_a).await;
        assert_eq!(
            refused.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{who}: {}",
            refused.body
        );
        assert_eq!(
            refused.body["code"], "service_account",
            "{who}: {}",
            refused.body
        );
    }
    assert!(
        orgs_of(&w.db, account).await.is_empty(),
        "a service account was made an org member"
    );

    // A frontline worker holds no `org_members` row either, so a bounded grant does not
    // reach one through enrolment.
    let crew = seed_frontline_worker(&w.db, w.org_a).await;
    let refused = add(&w.bounded, crew, w.org_a).await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND, "{}", refused.body);
    assert!(orgs_of(&w.db, crew).await.is_empty());
}
