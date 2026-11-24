//! Integration tests for the invitation expiry rule — the invariant behind a
//! permanent-lockout bug.
//!
//! Nothing ever transitions an `org_invitations` row from `pending` to
//! `expired`, so a lapsed invite keeps `status='pending'` forever. When the
//! create path deduped on `status` alone while the list path also required
//! `expires_at > now()`, one row read two different ways: it blocked every
//! future invite to that address with a 409, and it was invisible to the admin
//! who could have revoked it. There was no way out of it inside the product.
//!
//! These exercise the query-side rule (`live_pending` / `expired_pending`) and
//! the supersede delete against real Postgres. The handler wrappers around them
//! (`find_live_invitation` / `supersede_expired_invitations`) are `pub(crate)`
//! and so not reachable from an integration test; the conditions they run are.
//!
//! Skips automatically when `OXY_DATABASE_URL` is unset.
//!
//! To run locally:
//!   OXY_DATABASE_URL=postgres://... cargo nextest run -p oxy-api-tenancy --test integration -E 'test(org_invitations::)'

use std::sync::LazyLock;

use chrono::{Duration, Utc};
use entity::org_invitations::{self, InviteStatus};
use entity::org_members::OrgRole;
use entity::{organizations, users};
use oxy::database::client::establish_connection;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use tokio::runtime::Runtime;
use uuid::Uuid;

/// One runtime shared by every test in this file.
///
/// `establish_connection` memoizes its pool in a process-wide `OnceCell`, so
/// the pool belongs to whichever runtime first initialized it. With a
/// `#[tokio::test]` per case, cargo runs them on parallel threads with a
/// runtime each: the first to finish drops its runtime and every other test
/// inherits a dead pool ("A Tokio 1.x context was found, but it is being
/// shutdown"). A single long-lived runtime keeps the cached pool valid for the
/// whole run.
static RT: LazyLock<Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build test runtime")
});

fn db_unavailable() -> bool {
    std::env::var("OXY_DATABASE_URL").is_err()
}

async fn seed_user(email: &str) -> Uuid {
    let conn = establish_connection().await.expect("db connect");
    let user_id = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(user_id),
        email: ActiveValue::Set(Some(email.to_string())),
        name: ActiveValue::Set("Invite Test User".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        magic_link_token: ActiveValue::Set(None),
        magic_link_token_expires_at: ActiveValue::Set(None),
        status: ActiveValue::Set(users::UserStatus::Active),
        created_at: ActiveValue::NotSet,
        last_login_at: ActiveValue::NotSet,
    }
    .insert(&conn)
    .await
    .expect("seed user");
    user_id
}

async fn seed_org() -> Uuid {
    let conn = establish_connection().await.expect("db connect");
    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org_id),
        name: ActiveValue::Set(format!("Invite Test Org {org_id}")),
        slug: ActiveValue::Set(format!("invite-test-org-{org_id}")),
        logo: ActiveValue::NotSet,
        logo_content_type: ActiveValue::NotSet,
        created_at: ActiveValue::NotSet,
        updated_at: ActiveValue::NotSet,
    }
    .insert(&conn)
    .await
    .expect("seed org");
    org_id
}

/// Seed an invitation expiring `expires_in` from now. A negative duration
/// produces the lapsed-but-still-`pending` row that caused the lockout.
async fn seed_invitation(
    org_id: Uuid,
    invited_by: Uuid,
    email: &str,
    expires_in: Duration,
) -> Uuid {
    let conn = establish_connection().await.expect("db connect");
    let now = Utc::now().fixed_offset();
    let id = Uuid::new_v4();
    org_invitations::ActiveModel {
        id: ActiveValue::Set(id),
        org_id: ActiveValue::Set(org_id),
        email: ActiveValue::Set(email.to_string()),
        role: ActiveValue::Set(OrgRole::Member),
        invited_by: ActiveValue::Set(invited_by),
        token: ActiveValue::Set(Uuid::new_v4().to_string()),
        status: ActiveValue::Set(InviteStatus::Pending),
        expires_at: ActiveValue::Set(now + expires_in),
        created_at: ActiveValue::Set(now),
    }
    .insert(&conn)
    .await
    .expect("seed invitation");
    id
}

/// The invitation for `(org, email)` that can still be accepted, if any —
/// what the create path consults before rejecting a duplicate.
async fn find_live(org_id: Uuid, email: &str) -> Option<Uuid> {
    let conn = establish_connection().await.expect("db connect");
    org_invitations::Entity::find()
        .filter(org_invitations::Column::OrgId.eq(org_id))
        .filter(org_invitations::Column::Email.eq(email))
        .filter(org_invitations::live_pending(Utc::now().fixed_offset()))
        .one(&conn)
        .await
        .expect("query live invitation")
        .map(|inv| inv.id)
}

/// Delete lapsed rows for `(org, email)` — what an incoming invite does to
/// whatever it supersedes.
async fn supersede_expired(org_id: Uuid, email: &str) -> u64 {
    let conn = establish_connection().await.expect("db connect");
    org_invitations::Entity::delete_many()
        .filter(org_invitations::Column::OrgId.eq(org_id))
        .filter(org_invitations::Column::Email.eq(email))
        .filter(org_invitations::expired_pending(Utc::now().fixed_offset()))
        .exec(&conn)
        .await
        .expect("supersede expired")
        .rows_affected
}

/// Drop everything a test seeded. Invitations go with it: `org_invitations`
/// cascades on both `org_id` and `invited_by`.
///
/// CI runs against a throwaway service container, but these also run against
/// shared dev Postgres, where rows would otherwise pile up run after run. Best
/// effort and not panic-safe — a failing assertion leaves its rows behind,
/// which is the right trade for keeping the failure inspectable.
async fn cleanup(org_ids: &[Uuid], user_ids: &[Uuid]) {
    let conn = establish_connection().await.expect("db connect");
    for org_id in org_ids {
        organizations::Entity::delete_by_id(*org_id)
            .exec(&conn)
            .await
            .expect("cleanup org");
    }
    for user_id in user_ids {
        users::Entity::delete_by_id(*user_id)
            .exec(&conn)
            .await
            .expect("cleanup user");
    }
}

async fn invitation_exists(id: Uuid) -> bool {
    let conn = establish_connection().await.expect("db connect");
    org_invitations::Entity::find_by_id(id)
        .one(&conn)
        .await
        .expect("query invitation")
        .is_some()
}

/// The regression. A lapsed invite must not register as live, or it blocks its
/// own replacement forever — which is exactly what a 409 on re-invite meant.
#[test]
fn lapsed_invitation_does_not_block_a_new_one() {
    if db_unavailable() {
        eprintln!("Skipping: OXY_DATABASE_URL not set");
        return;
    }
    RT.block_on(async {
        let inviter = seed_user(&format!("inviter-{}@example.com", Uuid::new_v4().simple())).await;
        let org = seed_org().await;
        let invitee = format!("lapsed-{}@example.com", Uuid::new_v4().simple());

        let stale = seed_invitation(org, inviter, &invitee, Duration::days(-1)).await;

        // Still on the table, still `pending` — but not live, so it can't block.
        assert!(invitation_exists(stale).await);
        assert_eq!(find_live(org, &invitee).await, None);

        // The incoming invite clears it.
        assert_eq!(supersede_expired(org, &invitee).await, 1);
        assert!(!invitation_exists(stale).await);

        cleanup(&[org], &[inviter]).await;
    });
}

/// The other half: a still-usable invite must keep blocking, so re-inviting
/// someone who already holds a working link doesn't mint a second token.
#[test]
fn live_invitation_still_blocks_and_survives_supersede() {
    if db_unavailable() {
        eprintln!("Skipping: OXY_DATABASE_URL not set");
        return;
    }
    RT.block_on(async {
        let inviter = seed_user(&format!("inviter-{}@example.com", Uuid::new_v4().simple())).await;
        let org = seed_org().await;
        let invitee = format!("live-{}@example.com", Uuid::new_v4().simple());

        let live = seed_invitation(org, inviter, &invitee, Duration::days(7)).await;

        assert_eq!(find_live(org, &invitee).await, Some(live));
        // Superseding must not touch it.
        assert_eq!(supersede_expired(org, &invitee).await, 0);
        assert!(invitation_exists(live).await);

        cleanup(&[org], &[inviter]).await;
    });
}

/// Supersede is scoped to one `(org, email)` pair. A shared address invited to
/// several orgs — the real shape of the production data — must not have another
/// org's invitations collected as collateral.
#[test]
fn supersede_is_scoped_to_one_org_and_email() {
    if db_unavailable() {
        eprintln!("Skipping: OXY_DATABASE_URL not set");
        return;
    }
    RT.block_on(async {
        let inviter = seed_user(&format!("inviter-{}@example.com", Uuid::new_v4().simple())).await;
        let org_a = seed_org().await;
        let org_b = seed_org().await;
        let shared = format!("shared-{}@example.com", Uuid::new_v4().simple());
        let other = format!("other-{}@example.com", Uuid::new_v4().simple());

        let stale_a = seed_invitation(org_a, inviter, &shared, Duration::days(-1)).await;
        let stale_b = seed_invitation(org_b, inviter, &shared, Duration::days(-1)).await;
        let stale_other = seed_invitation(org_a, inviter, &other, Duration::days(-1)).await;

        assert_eq!(supersede_expired(org_a, &shared).await, 1);

        assert!(!invitation_exists(stale_a).await);
        assert!(invitation_exists(stale_b).await, "other org untouched");
        assert!(
            invitation_exists(stale_other).await,
            "other email untouched"
        );

        cleanup(&[org_a, org_b], &[inviter]).await;
    });
}

/// An accepted invitation is not pending, so neither condition may claim it —
/// superseding one would erase the record of how a member got their role.
#[test]
fn accepted_invitation_is_neither_live_nor_superseded() {
    if db_unavailable() {
        eprintln!("Skipping: OXY_DATABASE_URL not set");
        return;
    }
    RT.block_on(async {
        let conn = establish_connection().await.expect("db connect");
        let inviter = seed_user(&format!("inviter-{}@example.com", Uuid::new_v4().simple())).await;
        let org = seed_org().await;
        let invitee = format!("accepted-{}@example.com", Uuid::new_v4().simple());

        // Accepted, and old enough that a status-blind expiry check would sweep it.
        let id = seed_invitation(org, inviter, &invitee, Duration::days(-30)).await;
        let mut active: org_invitations::ActiveModel = org_invitations::Entity::find_by_id(id)
            .one(&conn)
            .await
            .expect("query")
            .expect("row")
            .into();
        active.status = ActiveValue::Set(InviteStatus::Accepted);
        active.update(&conn).await.expect("mark accepted");

        assert_eq!(find_live(org, &invitee).await, None);
        assert_eq!(supersede_expired(org, &invitee).await, 0);
        assert!(invitation_exists(id).await);

        cleanup(&[org], &[inviter]).await;
    });
}
