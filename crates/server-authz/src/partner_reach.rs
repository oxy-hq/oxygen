//! A partner's reach over one client, for one capability: the two facts
//! publish authorization reads from the partner's scope (C3a, 2026-11-28).
//!
//! `oxy_custom_apps::publish_authz::resolve_actor` composed `partner_for_org`
//! and `resolve_scope` itself and read `scope.org_ids.contains(target)` and
//! `scope.allows(ManageApps)`. [`partner_reach`] is that composition, moved here
//! so the enforcement port can answer it without the port naming
//! [`PartnerScope`]. It reads the partner's **ceiling** only, with no token
//! term: it is deliberately not [`crate::partner_allows`], which adds the
//! token's rules and would change who may publish.

use contracts_authz::PartnerReach;
use oxy_authz::Cap;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use crate::caller::Caller;
use crate::partner_authz::{PartnerCapability, PartnerScope, partner_for_org, resolve_scope};

/// The reach of the partner that manages `org_id`, for `cap`. `None` when no
/// partner manages the org or the caller resolves no scope in it, exactly
/// where publish fell through to "outsider".
pub async fn partner_reach(
    db: &DatabaseConnection,
    caller: &Caller,
    org_id: Uuid,
    cap: Cap,
) -> Option<PartnerReach> {
    let partner_org_id = partner_for_org(db, org_id).await?;
    let scope = resolve_scope(db, partner_org_id, caller).await?;
    Some(reach_of(&scope, org_id, cap))
}

/// The pure half of [`partner_reach`]: what one resolved scope gives over `org_id`.
fn reach_of(scope: &PartnerScope, org_id: Uuid, cap: Cap) -> PartnerReach {
    PartnerReach {
        manages_target: scope.org_ids.contains(&org_id),
        holds_cap: partner_capability_of(cap).is_some_and(|c| scope.allows(c)),
    }
}

/// The partner capability a [`Cap`] is, if any: the inverse of [`crate::cap_of`].
/// A platform-only capability has no partner analogue, so no partner holds it.
fn partner_capability_of(cap: Cap) -> Option<PartnerCapability> {
    match cap {
        Cap::ManageMembers => Some(PartnerCapability::ManageMembers),
        Cap::ManageApps => Some(PartnerCapability::ManageApps),
        Cap::DevelopApps => Some(PartnerCapability::DevelopApps),
        Cap::ViewAudit => Some(PartnerCapability::ViewAudit),
        Cap::ManageBilling => Some(PartnerCapability::ManageBilling),
        Cap::ManageSecrets => Some(PartnerCapability::ManageSecrets),
        Cap::CreateOrgs => Some(PartnerCapability::CreateOrgs),
        Cap::ManageOrgSettings => Some(PartnerCapability::ManageOrgSettings),
        Cap::ViewTenants
        | Cap::ManagePartners
        | Cap::OperatePlatform
        | Cap::ManagePlatformGrants
        | Cap::CheckApps => None,
    }
}

#[cfg(test)]
#[path = "partner_reach_tests.rs"]
mod tests;
