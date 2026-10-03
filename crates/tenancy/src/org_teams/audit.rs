//! Audit rows for this module's access-conferring writes.
//!
//! The org route is the third door onto [`super::service::write_access`], and it
//! was the only one of the three that recorded nothing — while the launcher's
//! Access button made it the path an operator is *most* likely to take. Its two
//! siblings both audit (`admin.app.access_changed`, `partner.app.access_changed`),
//! and a change that differs only by which door it came through must not differ in
//! whether the org can see it happened.
//!
//! Team writes record too, for the same reason the access write does. A team is an
//! access-conferring object: `has_app_grant` reaches `app_team_grants` **through**
//! `org_team_members`, so adding somebody to a granted team hands them the app
//! exactly as a direct grant would. Auditing the grant but not the roster would
//! leave the log defeatable in one step — grant a team once, then move people into
//! it quietly.

use oxy_auth::types::AuthenticatedUser;
use oxy_server_authz::org_context::OrgContext;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use oxy_app_core::audit::{self, ActorType, AuditEntry};

/// The two names one change can land under.
///
/// Same event, different actor. A Global Owner or Global Admin who is not a real
/// member reaches these routes through a synthesized Owner membership
/// (`OrgContext::is_global_override`), and records under the `admin.` prefix their
/// own console uses. Without the split, the org's log would attribute Oxy's writes
/// to the org's own admins — and the reader cannot tell them apart by cross-
/// referencing `org_members`, because staff never appear there.
pub struct AccessAction {
    tenant: &'static str,
    staff: &'static str,
}

/// Deliberately the same name `/admin/apps/{id}/access` already records: one event,
/// two doors. A reader filtering the log for "who changed who can see this app"
/// must not have to know which route the operator happened to use.
pub const APP_ACCESS_CHANGED: AccessAction = AccessAction {
    tenant: "app.access_changed",
    staff: "admin.app.access_changed",
};

pub const TEAM_CREATED: AccessAction = AccessAction {
    tenant: "team.created",
    staff: "admin.team.created",
};

pub const TEAM_UPDATED: AccessAction = AccessAction {
    tenant: "team.updated",
    staff: "admin.team.updated",
};

/// Worth its own name rather than folding into `team.updated`: deleting a team
/// cascades `app_team_grants`, so it silently revokes every app the team reached.
pub const TEAM_DELETED: AccessAction = AccessAction {
    tenant: "team.deleted",
    staff: "admin.team.deleted",
};

pub const TEAM_MEMBER_ADDED: AccessAction = AccessAction {
    tenant: "team.member_added",
    staff: "admin.team.member_added",
};

pub const TEAM_MEMBER_REMOVED: AccessAction = AccessAction {
    tenant: "team.member_removed",
    staff: "admin.team.member_removed",
};

/// What the row points at: `("app" | "team", id, human label)`.
pub type Target = (&'static str, Uuid, String);

/// Record one access change into the org's append-only log.
///
/// Best-effort, matching both siblings: an audit write that fails must not fail the
/// request that already succeeded, or a transient log problem becomes an outage on
/// the control plane.
pub async fn record(
    db: &DatabaseConnection,
    ctx: &OrgContext,
    actor: &AuthenticatedUser,
    action: AccessAction,
    target: Target,
) {
    let name = if ctx.is_global_override {
        action.staff
    } else {
        action.tenant
    };
    let (kind, id, label) = target;
    audit::record_best_effort(
        db,
        AuditEntry::new(actor.label().to_string(), name)
            // `User` on both branches, matching the admin sibling — the actor tier
            // is carried by the action prefix, not by re-typing the actor.
            .actor(actor.id, ActorType::User)
            .org(ctx.org.id)
            .target(kind, id.to_string(), label),
    )
    .await;
}
