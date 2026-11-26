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

//!
//! This module decides **what** a change records: its name, its org and its
//! target ([`event`]). Writing the row is the caller's: the audit writer
//! (`oxy_app_core::audit`) sits above this crate, so each surface that records
//! one (`oxy-api-tenancy`'s `org_teams::audit::record`, `oxy-api-frontline`'s
//! `access_audit::record_app_access_changed`) writes the [`AccessEvent`] it
//! gets back (`internal-docs/crate-tiers.md`, stage A).

use oxy_server_authz::org_context::OrgContext;
use uuid::Uuid;

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

/// One access change, resolved to the row it records: everything but the
/// actor, which the writer takes from the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessEvent {
    /// The action's name for this actor tier ([`AccessAction`]).
    pub name: &'static str,
    /// The org whose log the row lands in.
    pub org_id: Uuid,
    /// What the row points at.
    pub target: Target,
}

/// The row one access change records into the org's append-only log.
///
/// The writer records it best-effort, matching both siblings: an audit write
/// that fails must not fail the request that already succeeded, or a transient
/// log problem becomes an outage on the control plane. The actor type is `User`
/// on both branches, matching the admin sibling: the actor tier is carried by
/// the action prefix, not by re-typing the actor.
pub fn event(ctx: &OrgContext, action: AccessAction, target: Target) -> AccessEvent {
    let name = if ctx.is_global_override {
        action.staff
    } else {
        action.tenant
    };
    AccessEvent {
        name,
        org_id: ctx.org.id,
        target,
    }
}
