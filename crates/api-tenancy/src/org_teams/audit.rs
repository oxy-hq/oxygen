//! The audit rows this surface's access-conferring writes leave in the org's
//! log. What a change records (its name, org and target) is the domain's,
//! [`oxy_tenancy::org_teams::audit::event`]; this writes it.

use oxy_app_core::audit::{self, AuditEntry, RequestActor};
use oxy_server_authz::org_context::OrgContext;
use oxy_tenancy::org_teams::audit::AuditActor;
use sea_orm::DatabaseConnection;

pub use oxy_tenancy::org_teams::audit::{
    APP_ACCESS_CHANGED, AccessAction, TEAM_CREATED, TEAM_DELETED, TEAM_MEMBER_ADDED,
    TEAM_MEMBER_REMOVED, TEAM_UPDATED, Target,
};

/// Record one access change into the org's append-only log, best-effort: an
/// audit write that fails must not fail the request that already succeeded.
pub async fn record(
    db: &DatabaseConnection,
    ctx: &OrgContext,
    actor: &RequestActor,
    action: AccessAction,
    target: Target,
) {
    let event = oxy_tenancy::org_teams::audit::event(
        AuditActor {
            org_id: ctx.org.id,
            is_global_override: ctx.is_global_override,
        },
        action,
        target,
    );
    let (kind, id, label) = event.target;
    audit::record_best_effort(
        db,
        AuditEntry::for_request(actor, event.name)
            .org(event.org_id)
            .target(kind, id.to_string(), label),
    )
    .await;
}
