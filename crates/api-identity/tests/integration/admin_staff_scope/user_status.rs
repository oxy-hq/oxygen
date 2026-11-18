//! `PATCH /admin/users/{id}/status` — the one user write whose reach is every org the
//! user belongs to.
//!
//! `deleted` refuses the person on every protected route and at every login, in every
//! org at once. So the write is fenced three ways: a bounded grant reaches a user only
//! when it reaches **every** org they are in (a user in no org is platform-level), the
//! Global Owner cannot be locked out from the console at all, and a staff target is
//! behind the same delegation bound as their grant row — no peer, nothing wider than
//! the writer's own standing. A change is audited in its own transaction.

use axum::Json;
use axum::extract::{OriginalUri, Path, Query};
use axum::http::{StatusCode, Uri};
use entity::audit_events;
use entity::org_members::OrgRole;
use entity::users::{self, UserStatus};
use oxy_api_identity::admin::users_admin::{SetStatusBody, set_user_status};
use oxy_app::surface::testing::audit::{AuditQuery, list_audit};
use oxy_app_core::audit::RequestActor;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseBackend,
    DatabaseConnection, DatabaseTransaction, EntityTrait, QueryFilter, QuerySelect, Statement,
    TransactionTrait,
};
use serde_json::json;
use std::time::Duration;
use uuid::Uuid;

use super::fixture::{Reply, World, as_actor, grant, grant_as, member_of, reply, seed_user, world};

async fn set_status(actor: &AuthenticatedUser, user: Uuid, status: &str) -> Reply {
    let body = Json(SetStatusBody {
        status: status.to_string(),
    });
    let actor = RequestActor::session(actor.clone());
    reply(set_user_status(actor, Path(user), body).await).await
}

async fn status_of(db: &DatabaseConnection, user: Uuid) -> UserStatus {
    users::Entity::find_by_id(user)
        .one(db)
        .await
        .expect("read user")
        .expect("user exists")
        .status
}

async fn audit_rows(db: &DatabaseConnection, user: Uuid) -> Vec<audit_events::Model> {
    audit_events::Entity::find()
        .filter(audit_events::Column::TargetId.eq(user.to_string()))
        .all(db)
        .await
        .expect("read audit events")
}

#[tokio::test]
async fn a_bounded_grant_cannot_lock_out_a_user_who_belongs_past_its_orgs() {
    let w = world().await;
    let in_b = seed_user(&w.db, "in-b@tenant.test").await;
    member_of(&w.db, in_b.id, w.org_b, OrgRole::Member).await;
    let in_both = seed_user(&w.db, "in-both@tenant.test").await;
    member_of(&w.db, in_both.id, w.org_a, OrgRole::Member).await;
    member_of(&w.db, in_both.id, w.org_b, OrgRole::Member).await;
    let in_none = seed_user(&w.db, "in-none@tenant.test").await;

    let missing = set_status(&w.bounded, Uuid::new_v4(), "deleted").await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    for (who, target) in [
        ("a member of org B only", &in_b),
        ("a member of orgs A and B", &in_both),
        ("a user in no org", &in_none),
    ] {
        let refused = set_status(&w.bounded, target.id, "deleted").await;
        assert_eq!(refused.status, StatusCode::NOT_FOUND, "{who}");
        assert_eq!(refused.body, missing.body, "{who}");
        assert_eq!(
            status_of(&w.db, target.id).await,
            UserStatus::Active,
            "{who} was locked out by a grant bounded to org A"
        );
    }

    let in_a = seed_user(&w.db, "in-a@tenant.test").await;
    member_of(&w.db, in_a.id, w.org_a, OrgRole::Member).await;
    let done = set_status(&w.bounded, in_a.id, "deleted").await;
    assert_eq!(done.status, StatusCode::NO_CONTENT, "{}", done.body);
    assert_eq!(status_of(&w.db, in_a.id).await, UserStatus::Deleted);

    for (who, actor) in w.everything_readers() {
        for target in [&in_b, &in_both, &in_none] {
            let done = set_status(actor, target.id, "deleted").await;
            assert_eq!(done.status, StatusCode::NO_CONTENT, "{who}: {}", done.body);
            let done = set_status(actor, target.id, "active").await;
            assert_eq!(done.status, StatusCode::NO_CONTENT, "{who}: {}", done.body);
        }
    }
}

#[tokio::test]
async fn nobody_can_lock_the_global_owner_out_from_the_console() {
    let w = world().await;
    for (who, actor) in w.everything_readers() {
        let refused = set_status(actor, w.owner.id, "deleted").await;
        assert_eq!(refused.status, StatusCode::FORBIDDEN, "{who}");
        assert_eq!(
            refused.body["code"], "global_owner",
            "{who}: {}",
            refused.body
        );
        assert_eq!(
            status_of(&w.db, w.owner.id).await,
            UserStatus::Active,
            "{who} locked the Global Owner out"
        );
    }
    // A bounded grant is not told root exists: the owner is in no org it reaches.
    let refused = set_status(&w.bounded, w.owner.id, "deleted").await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);

    // Reactivating root restores only what `OXY_OWNER` already grants, and it is the way
    // back for an account suspended before this fence existed.
    let mut owner: users::ActiveModel = users::Entity::find_by_id(w.owner.id)
        .one(&w.db)
        .await
        .expect("read owner")
        .expect("owner exists")
        .into();
    owner.status = ActiveValue::Set(UserStatus::Deleted);
    owner.update(&w.db).await.expect("suspend owner by hand");
    let restored = set_status(&w.unbounded, w.owner.id, "active").await;
    assert_eq!(restored.status, StatusCode::NO_CONTENT, "{}", restored.body);
    assert_eq!(status_of(&w.db, w.owner.id).await, UserStatus::Active);
}

#[tokio::test]
async fn a_global_admin_cannot_lock_out_a_peer_or_a_grant_wider_than_its_own() {
    let w = world().await;
    // Both Global Admins are members of org A, so scope is not what refuses them.
    member_of(&w.db, w.bounded.id, w.org_a, OrgRole::Member).await;
    member_of(&w.db, w.unbounded.id, w.org_a, OrgRole::Member).await;

    for (who, actor, target) in [
        (
            "an all-orgs admin over a bounded peer",
            &w.unbounded,
            &w.bounded,
        ),
        (
            "a bounded admin over an all-orgs peer",
            &w.bounded,
            &w.unbounded,
        ),
        ("an admin over its own account", &w.unbounded, &w.unbounded),
    ] {
        let refused = set_status(actor, target.id, "deleted").await;
        assert_eq!(refused.status, StatusCode::FORBIDDEN, "{who}");
        assert_eq!(
            refused.body["denial"], "role_not_below",
            "{who}: {}",
            refused.body
        );
        assert_eq!(
            status_of(&w.db, target.id).await,
            UserStatus::Active,
            "{who}"
        );
    }

    // An App Operator whose grant reaches past the writer's own scope.
    let wide = seed_user(&w.db, "op-all@staff-scope.test").await;
    grant_as(&w.db, "op-all@staff-scope.test", "app_operator", None).await;
    member_of(&w.db, wide.id, w.org_a, OrgRole::Member).await;
    let refused = set_status(&w.bounded, wide.id, "deleted").await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert_eq!(
        refused.body["denial"], "scope_not_contained",
        "{}",
        refused.body
    );
    assert_eq!(status_of(&w.db, wide.id).await, UserStatus::Active);

    // One inside it is the writer's to manage.
    let narrow = seed_user(&w.db, "op-a@staff-scope.test").await;
    grant_as(
        &w.db,
        "op-a@staff-scope.test",
        "app_operator",
        Some(&[w.org_a]),
    )
    .await;
    member_of(&w.db, narrow.id, w.org_a, OrgRole::Member).await;
    let done = set_status(&w.bounded, narrow.id, "deleted").await;
    assert_eq!(done.status, StatusCode::NO_CONTENT, "{}", done.body);

    // Who is staff at the top tier stays the Owner's call.
    let done = set_status(&w.owner, w.bounded.id, "deleted").await;
    assert_eq!(done.status, StatusCode::NO_CONTENT, "{}", done.body);
    assert_eq!(status_of(&w.db, w.bounded.id).await, UserStatus::Deleted);
}

/// Wait until a backend on this test's own database is blocked on a lock: the status
/// write, queued behind the row lock the test holds.
async fn until_a_write_waits_on_the_row(db: &DatabaseConnection) {
    let waiting = Statement::from_string(
        DatabaseBackend::Postgres,
        "SELECT count(*)::bigint AS n FROM pg_stat_activity \
         WHERE datname = current_database() AND wait_event_type = 'Lock'",
    );
    for _ in 0..400 {
        let row = db
            .query_one_raw(waiting.clone())
            .await
            .expect("read pg_stat_activity")
            .expect("one row");
        if row.try_get::<i64>("", "n").expect("count") > 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the status write never queued behind the row lock");
}

/// Run a suspension of `target` by `actor` while the test holds `target`'s row the way a
/// concurrent writer would, and make `meanwhile` happen in that holding transaction
/// once the suspension is queued behind it.
async fn suspend_while<F>(w: &World, actor: &AuthenticatedUser, target: Uuid, meanwhile: F) -> Reply
where
    F: AsyncFnOnce(&DatabaseTransaction),
{
    let holder = w.db.begin().await.expect("begin");
    users::Entity::find_by_id(target)
        .lock_exclusive()
        .one(&holder)
        .await
        .expect("lock the target's row")
        .expect("target exists");
    let actor = actor.clone();
    let write = tokio::spawn(async move { set_status(&actor, target, "deleted").await });
    until_a_write_waits_on_the_row(&w.db).await;
    meanwhile(&holder).await;
    holder.commit().await.expect("commit");
    write.await.expect("the status write finished")
}

/// The decision is made on what holds when the row is written, not on what held a
/// moment before. The checks above read memberships and the target's grant before the
/// write locks the row; a change that lands in between must be decided on.
///
/// Here the target joins org B while the write waits: they are no longer the grant's.
#[tokio::test]
async fn a_status_change_rereads_memberships_under_its_lock() {
    let w = world().await;
    let joining = seed_user(&w.db, "joining@tenant.test").await;
    member_of(&w.db, joining.id, w.org_a, OrgRole::Member).await;
    let org_b = w.org_b;
    let refused = suspend_while(&w, &w.bounded, joining.id, async |tx| {
        member_of(tx, joining.id, org_b, OrgRole::Member).await;
    })
    .await;
    assert_eq!(
        status_of(&w.db, joining.id).await,
        UserStatus::Active,
        "a grant bounded to org A suspended a user who joined org B while the write waited ({})",
        refused.status
    );
    assert_eq!(refused.status, StatusCode::NOT_FOUND, "{}", refused.body);
    assert!(audit_rows(&w.db, joining.id).await.is_empty());
}

/// As above, with the target made a Global Admin while the write waits: now a peer.
#[tokio::test]
async fn a_status_change_rereads_the_targets_grant_under_its_lock() {
    let w = world().await;
    let promoted = seed_user(&w.db, "promoted@staff-scope.test").await;
    member_of(&w.db, promoted.id, w.org_a, OrgRole::Member).await;
    let refused = suspend_while(&w, &w.unbounded, promoted.id, async |tx| {
        grant(tx, "promoted@staff-scope.test", None).await;
    })
    .await;
    assert_eq!(
        status_of(&w.db, promoted.id).await,
        UserStatus::Active,
        "an all-orgs admin suspended a peer made Global Admin while the write waited ({})",
        refused.status
    );
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    assert_eq!(refused.body["denial"], "role_not_below", "{}", refused.body);
}

/// `user.status.updated` rows, as `actor`'s `GET /admin/audit` lists them.
async fn status_events_as(actor: &AuthenticatedUser) -> Reply {
    let query = Query(AuditQuery {
        action: Some("user.status.updated".into()),
        actor: None,
        org_id: None,
        outcome: None,
        q: None,
        token_id: None,
        limit: None,
        offset: None,
    });
    let uri = OriginalUri(Uri::from_static("/api/admin/audit"));
    reply(list_audit(as_actor(actor), uri, query).await).await
}

/// A suspension takes effect in every org the person is in, so each of those orgs'
/// audit trail records it: one row per org, sharing a `metadata.event_id`. A person in
/// no org gets one row with no org, as before.
#[tokio::test]
async fn a_status_change_is_audited_in_every_org_of_the_target() {
    let w = world().await;
    let shared = seed_user(&w.db, "shared@tenant.test").await;
    member_of(&w.db, shared.id, w.org_a, OrgRole::Member).await;
    member_of(&w.db, shared.id, w.org_b, OrgRole::Member).await;
    let done = set_status(&w.unbounded, shared.id, "deleted").await;
    assert_eq!(done.status, StatusCode::NO_CONTENT, "{}", done.body);

    let rows = audit_rows(&w.db, shared.id).await;
    let mut orgs: Vec<Option<Uuid>> = rows.iter().map(|r| r.org_id).collect();
    orgs.sort();
    let mut want = vec![Some(w.org_a), Some(w.org_b)];
    want.sort();
    assert_eq!(orgs, want, "one row in each org of the target: {rows:?}");
    let event_ids: Vec<&serde_json::Value> = rows.iter().map(|r| &r.metadata["event_id"]).collect();
    assert!(event_ids[0].is_string(), "{rows:?}");
    assert_eq!(event_ids[0], event_ids[1], "the rows are one event");

    // A grant bounded to org A sees org A's row of it, and only that one.
    let a_row = rows
        .iter()
        .find(|r| r.org_id == Some(w.org_a))
        .expect("org A row");
    let seen = status_events_as(&w.bounded).await;
    assert_eq!(seen.column(None, "id"), vec![a_row.id.to_string()]);

    let loner = seed_user(&w.db, "loner@tenant.test").await;
    let done = set_status(&w.unbounded, loner.id, "deleted").await;
    assert_eq!(done.status, StatusCode::NO_CONTENT, "{}", done.body);
    let rows = audit_rows(&w.db, loner.id).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].org_id, None);
}

#[tokio::test]
async fn a_status_change_is_audited_and_a_refused_one_writes_nothing() {
    let w = world().await;
    let target = seed_user(&w.db, "audited@tenant.test").await;
    member_of(&w.db, target.id, w.org_a, OrgRole::Member).await;
    let done = set_status(&w.bounded, target.id, "deleted").await;
    assert_eq!(done.status, StatusCode::NO_CONTENT, "{}", done.body);

    let rows = audit_rows(&w.db, target.id).await;
    assert_eq!(rows.len(), 1, "one status change, one audit row: {rows:?}");
    let row = &rows[0];
    assert_eq!(row.action, "user.status.updated");
    assert_eq!(row.actor_user_id, Some(w.bounded.id));
    assert_eq!(row.target_type.as_deref(), Some("user"));
    assert_eq!(row.before, Some(json!({ "status": "active" })));
    assert_eq!(row.after, Some(json!({ "status": "deleted" })));
    assert_eq!(
        row.org_id,
        Some(w.org_a),
        "the row belongs to the target's org"
    );

    // So the bounded grant finds its own action in its own, org-scoped, audit view.
    let mine = status_events_as(&w.bounded).await;
    assert_eq!(mine.status, StatusCode::OK);
    assert_eq!(mine.column(None, "id"), vec![row.id.to_string()]);

    let outside = seed_user(&w.db, "outside@tenant.test").await;
    member_of(&w.db, outside.id, w.org_b, OrgRole::Member).await;
    let refused = set_status(&w.bounded, outside.id, "deleted").await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);
    assert!(audit_rows(&w.db, outside.id).await.is_empty());
}
