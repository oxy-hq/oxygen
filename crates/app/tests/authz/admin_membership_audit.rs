//! Every staff change to org membership must land in `audit_events`.
//!
//! `POST/PATCH/DELETE /admin/users/{id}/org-memberships` put a person into a tenant,
//! change their standing inside it, or take it away. Until this test's companion
//! change, all three wrote **nothing** to the audit log — the partner tier had recorded
//! the equivalent actions since it shipped, the staff path simply never did. There was
//! no record of who was added where, by whom, or at what role.
//!
//! The audit row is written inside the handler's own transaction, so the property here
//! is stronger than "a row appears": a membership change that cannot be recorded must
//! not commit. These tests exercise the real handlers against a real database rather
//! than asserting on source, because the thing worth pinning is the *transaction*, and
//! no amount of grepping proves a commit boundary.
//!
//! Skips when `OXY_DATABASE_URL` is unset. To run:
//!   OXY_DATABASE_URL=postgres://... cargo nextest run -p oxy-app --test authz -E 'test(admin_membership_audit)'

use axum::Json;
use axum::extract::Path;
use entity::org_members::OrgRole;
use entity::{audit_events, org_members, organizations, users};
use oxy::database::client::establish_connection;
use oxy_app::server::api::admin::users_admin::{
    AddToOrgBody, UpdateRoleBody, add_to_org, remove_from_org, update_role,
};
use oxy_app_core::audit::RequestActor;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder,
};
use uuid::Uuid;

fn db_unavailable() -> bool {
    std::env::var("OXY_DATABASE_URL").is_err()
}

async fn seed_user(conn: &DatabaseConnection, tag: &str) -> users::Model {
    let id = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(Some(format!("{tag}-{id}@audit.test"))),
        name: ActiveValue::Set(format!("Audit {tag}")),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        magic_link_token: ActiveValue::Set(None),
        magic_link_token_expires_at: ActiveValue::Set(None),
        status: ActiveValue::Set(users::UserStatus::Active),
        created_at: ActiveValue::NotSet,
        last_login_at: ActiveValue::NotSet,
    }
    .insert(conn)
    .await
    .expect("seed user")
}

async fn seed_org(conn: &DatabaseConnection) -> organizations::Model {
    let id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(id),
        name: ActiveValue::Set(format!("Audit Org {id}")),
        slug: ActiveValue::Set(format!("audit-{id}")),
        logo: ActiveValue::NotSet,
        logo_content_type: ActiveValue::NotSet,
        created_at: ActiveValue::NotSet,
        updated_at: ActiveValue::NotSet,
    }
    .insert(conn)
    .await
    .expect("seed org")
}

fn actor_of(u: &users::Model) -> RequestActor {
    RequestActor::session(AuthenticatedUser {
        id: u.id,
        email: u.email.clone(),
        name: u.name.clone(),
        picture: u.picture.clone(),
        status: u.status.clone(),
        credential: None,
    })
}

/// Audit rows for one org, oldest first.
async fn events_for(conn: &DatabaseConnection, org_id: Uuid) -> Vec<audit_events::Model> {
    audit_events::Entity::find()
        .filter(audit_events::Column::OrgId.eq(org_id))
        .order_by_asc(audit_events::Column::CreatedAt)
        .all(conn)
        .await
        .expect("load audit events")
}

/// The whole lifecycle in one test, because the interesting assertions are about the
/// SEQUENCE: add → change → remove has to read as a story, and the `before`/`after`
/// payloads are what make it one.
#[tokio::test]
async fn staff_membership_changes_are_audited_end_to_end() {
    if db_unavailable() {
        eprintln!("skipping: OXY_DATABASE_URL unset");
        return;
    }
    let conn = establish_connection().await.expect("db connect");

    let staff = seed_user(&conn, "staff").await;
    let target = seed_user(&conn, "target").await;
    let org = seed_org(&conn).await;
    // A second owner, so the last-owner guard doesn't block the demote/remove below.
    let other = seed_user(&conn, "owner").await;
    add_to_org(
        actor_of(&staff),
        Path(other.id),
        Json(AddToOrgBody {
            org_id: org.id,
            role: "owner".into(),
        }),
    )
    .await
    .expect("seed second owner");

    // ── add ──────────────────────────────────────────────────────────────────
    add_to_org(
        actor_of(&staff),
        Path(target.id),
        Json(AddToOrgBody {
            org_id: org.id,
            role: "admin".into(),
        }),
    )
    .await
    .expect("add_to_org");

    // ── change role ──────────────────────────────────────────────────────────
    update_role(
        actor_of(&staff),
        Path((target.id, org.id)),
        Json(UpdateRoleBody {
            role: "member".into(),
        }),
    )
    .await
    .expect("update_role");

    // ── remove ───────────────────────────────────────────────────────────────
    remove_from_org(actor_of(&staff), Path((target.id, org.id)))
        .await
        .expect("remove_from_org");

    let events = events_for(&conn, org.id).await;
    let actions: Vec<&str> = events.iter().map(|e| e.action.as_str()).collect();
    assert_eq!(
        actions,
        vec![
            "member.added", // the second owner
            "member.added", // the target
            "member.role.updated",
            "member.removed",
        ],
        "every staff membership change must leave exactly one audit row, in order"
    );

    // The rows must be readable by a human under pressure, which means the actor and
    // the target are named — not just their uuids.
    for e in &events {
        assert_eq!(Some(e.actor_email.clone()), staff.email, "actor recorded");
        assert_eq!(e.actor_type, "user");
        assert_eq!(e.outcome, "success");
    }

    let target_events: Vec<&audit_events::Model> = events
        .iter()
        .filter(|e| e.target_id.as_deref() == Some(target.id.to_string().as_str()))
        .collect();
    assert_eq!(target_events.len(), 3, "three rows about the target");
    assert!(
        target_events.iter().all(|e| e.target_label == target.email),
        "the target is labelled by email — a log keyed only by uuid is unreadable \
         exactly when someone needs to read it"
    );

    // before/after carry the role transition. Without them, "removed" cannot answer
    // whether an owner or a viewer was taken out, which is the question that matters.
    let added = target_events[0];
    assert!(added.before.is_none() || added.before == Some(serde_json::Value::Null));
    assert_eq!(added.after, Some(serde_json::json!({ "role": "admin" })));

    let changed = target_events[1];
    assert_eq!(changed.before, Some(serde_json::json!({ "role": "admin" })));
    assert_eq!(changed.after, Some(serde_json::json!({ "role": "member" })));

    let removed = target_events[2];
    assert_eq!(
        removed.before,
        Some(serde_json::json!({ "role": "member" }))
    );
    assert!(removed.after.is_none() || removed.after == Some(serde_json::Value::Null));
}

/// A rejected change must leave NO audit row.
///
/// The membership write and its audit row share a transaction, so a refusal rolls both
/// back. Worth pinning explicitly: an audit log that records attempts as though they
/// succeeded is worse than none, because it is believed.
#[tokio::test]
async fn a_refused_membership_change_writes_no_audit_row() {
    if db_unavailable() {
        eprintln!("skipping: OXY_DATABASE_URL unset");
        return;
    }
    let conn = establish_connection().await.expect("db connect");

    let staff = seed_user(&conn, "staff").await;
    let only_owner = seed_user(&conn, "solo").await;
    let org = seed_org(&conn).await;

    add_to_org(
        actor_of(&staff),
        Path(only_owner.id),
        Json(AddToOrgBody {
            org_id: org.id,
            role: "owner".into(),
        }),
    )
    .await
    .expect("seed the only owner");

    let before = events_for(&conn, org.id).await.len();

    // The last-owner guard refuses this.
    let demote = update_role(
        actor_of(&staff),
        Path((only_owner.id, org.id)),
        Json(UpdateRoleBody {
            role: "member".into(),
        }),
    )
    .await;
    assert!(demote.is_err(), "demoting the last owner must be refused");

    let removal = remove_from_org(actor_of(&staff), Path((only_owner.id, org.id))).await;
    assert!(removal.is_err(), "removing the last owner must be refused");

    assert_eq!(
        events_for(&conn, org.id).await.len(),
        before,
        "a refused change must not be recorded — a log that reports attempts as \
         successes is worse than no log, because it gets believed"
    );

    // And the membership itself is untouched.
    let still_owner = org_members::Entity::find()
        .filter(org_members::Column::OrgId.eq(org.id))
        .filter(org_members::Column::UserId.eq(only_owner.id))
        .one(&conn)
        .await
        .expect("query membership")
        .expect("membership survives");
    assert!(matches!(still_owner.role, OrgRole::Owner));
}
