//! [`partner_reach`]'s pure half against the two reads publish made before it
//! moved: `scope.org_ids.contains(&target)` and `scope.allows(ManageApps)`.
//!
//! The database half is `partner_for_org` then `resolve_scope`, composed as
//! publish composed them, with `?` standing for publish's fall-through: no
//! partner, or no scope, is `None` (an outsider).

use super::*;
use crate::partner_authz::Capabilities;

const TARGET: Uuid = Uuid::from_u128(1);
const OTHER: Uuid = Uuid::from_u128(2);

fn scope(org_ids: Vec<Uuid>, manage_apps: bool) -> PartnerScope {
    let mut capabilities = Capabilities::default();
    capabilities.set(PartnerCapability::ManageApps, manage_apps);
    PartnerScope {
        partner_id: Uuid::from_u128(9),
        slug: "partner".into(),
        capabilities,
        org_ids,
        token: None,
    }
}

/// What publish read from a resolved scope, before the move.
fn old_two_step(scope: &PartnerScope, target: Uuid) -> (bool, bool) {
    (
        scope.org_ids.contains(&target),
        scope.allows(PartnerCapability::ManageApps),
    )
}

#[test]
fn a_resolved_scope_answers_what_publish_read_from_it() {
    for scope in [
        // The scope does not cover the target, with and without the cap.
        scope(vec![OTHER], false),
        scope(vec![OTHER], true),
        // It covers the target without the cap, and with it.
        scope(vec![TARGET, OTHER], false),
        scope(vec![TARGET], true),
        // An empty managed set.
        scope(Vec::new(), true),
    ] {
        let reach = reach_of(&scope, TARGET, Cap::ManageApps);
        assert_eq!(
            (reach.manages_target, reach.holds_cap),
            old_two_step(&scope, TARGET),
            "{scope:?}"
        );
    }
}

/// Every partner capability maps back to itself, so `partner_reach(.., cap)` asks
/// the ceiling for exactly that capability.
#[test]
fn every_partner_capability_round_trips_through_cap() {
    for capability in PartnerCapability::ALL {
        assert_eq!(
            partner_capability_of(crate::cap_of(capability)),
            Some(capability)
        );
    }
}

/// A platform-only capability is held by no partner, whatever its ceiling.
#[test]
fn a_platform_only_cap_is_never_a_partners() {
    let mut everything = scope(vec![TARGET], true);
    for capability in PartnerCapability::ALL {
        everything.capabilities.set(capability, true);
    }
    for cap in [
        Cap::ViewTenants,
        Cap::ManagePartners,
        Cap::OperatePlatform,
        Cap::ManagePlatformGrants,
        Cap::CheckApps,
    ] {
        assert_eq!(partner_capability_of(cap), None, "{cap:?}");
        assert!(!reach_of(&everything, TARGET, cap).holds_cap, "{cap:?}");
    }
}
