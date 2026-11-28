//! The audit row a frontline grant change leaves in the org's log: the same
//! entry the access settings file (`app.access_changed`), so this door is not
//! the one way to reach an app that leaves no trail. What a change records is
//! the domain's, [`oxy_tenancy::org_teams::audit::event`]; this writes it.

use oxy_app_core::audit::{self, AuditEntry, RequestActor};
use oxy_server_authz::org_context::OrgContext;
use oxy_tenancy::org_teams::audit::{APP_ACCESS_CHANGED, AuditActor, Target};
use sea_orm::DatabaseConnection;

/// Record one app-access change, best-effort: an audit write that fails must
/// not fail the request that already succeeded.
pub(crate) async fn record_app_access_changed(
    db: &DatabaseConnection,
    ctx: &OrgContext,
    actor: &RequestActor,
    target: Target,
) {
    let event = oxy_tenancy::org_teams::audit::event(
        AuditActor {
            org_id: ctx.org.id,
            is_global_override: ctx.is_global_override,
        },
        APP_ACCESS_CHANGED,
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
