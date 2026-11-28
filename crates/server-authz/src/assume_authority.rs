//! Assume-role's **authority**: who may act as an org ([`may_act_as`]), and the
//! role a live session runs with ([`ActingAs`]).
//!
//! Moved down from `oxy_app_core::assume` (C2, 2026-11-28), which re-exports both,
//! so the enforcement port's implementation ([`crate::ServerAuthz`]) can answer it
//! without a dependency back on `oxy-app-core`. It reads only this crate's
//! platform-standing door and partner tier. The refusal of the staff console while
//! acting (`block_admin_while_acting`) stays in `oxy-app-core`: it opens its own
//! database connection through `oxy`'s client.
//!
//! Session liveness is [`crate::assume_liveness`]; the `/api/assume` routes and
//! their design are `oxy_api_admin::assume`.

use sea_orm::DatabaseConnection;
use uuid::Uuid;

pub use contracts_authz::ActingAs;

use crate::caller::Caller;

/// **Who may act as an org.** Two populations, one rule each:
///
/// * **Oxy staff** — any org. They always could (the synthetic-Owner seam);
///   assume-role just makes it deliberate and audited.
/// * **A partner** — only the clients they are assigned to, and only with
///   `develop_apps`. Entering a client's product IS the data plane: it means
///   reading their warehouse, their threads, their dashboards. An
///   `account_manager` — whose whole point is "runs the relationship, cannot touch
///   the data" — must not be able to walk in through this door, or the role split
///   is decorative.
///
/// Re-checked on **every request** (not just at session creation), so revoking a
/// partner's `develop_apps` or un-assigning a client kills a live session's reach
/// immediately rather than at expiry.
///
/// Takes the [`Caller`], so both authorities are what the request's credential
/// carries: an API token without `platform` is not staff here, and one without
/// `partner` operates no partner.
pub async fn may_act_as(
    db: &DatabaseConnection,
    caller: &Caller,
    org_id: Uuid,
) -> Option<ActingAs> {
    // Staff acting as a tenant synthesize **Owner** in that org, so this door demands the
    // capability that gates owner-level authority (`ManageOrgSettings`) — and honours the
    // grant's scope, since it is a reach INTO a specific tenant rather than a console
    // section.
    //
    // `is_staff()` alone would reopen exactly the hole the doc comment above warns about:
    // an App Operator holds neither capability, and their legitimate reach into a tenant
    // is the app data plane (`Ring::AppAccess` via `develop_apps`), which needs no
    // impersonation at all. A Global Admin holds `ManageOrgSettings` at `Scope::All`, so
    // nothing changes for them.
    if crate::globals::platform_reaches(db, caller, oxy_authz::Cap::ManageOrgSettings, org_id).await
    {
        return Some(ActingAs::Staff);
    }

    // A partner acting as one of its clients.
    use crate::partner_authz::{PartnerCapability, partner_for_org, resolve_scope};
    let partner_org_id = partner_for_org(db, org_id).await?;
    let scope = resolve_scope(db, partner_org_id, caller).await?;
    if !scope.allows(PartnerCapability::DevelopApps) {
        return None;
    }
    if !scope.org_ids.contains(&org_id) {
        return None;
    }
    Some(ActingAs::Partner)
}
