//! The delegation bound (`may_delegate`) and its door (`Action::PlatformGrants`):
//! who may write a platform grant row, and of what role and scope.

use super::*;
// The policy tests' fixtures.
use crate::policy_tests::{admin_over, facts, global_admin_standing, org};

// The delegation bound. Every assertion below is written so that deleting ONE
// guard in `may_delegate` reddens at least one of them — the file-wide
// `contains("may_delegate")` style of check is what let three earlier defects
// through on this branch.

#[test]
fn owner_may_delegate_anything_including_a_peer_tier_grant() {
    let go = PrincipalFacts {
        is_global_owner: true,
        ..facts()
    };
    // Root, and holding no row: the short-circuit is load-bearing, because a rank
    // lookup for the owner finds nothing.
    assert_eq!(
        may_delegate(&go, PlatformRole::GlobalAdmin, &Scope::All),
        Ok(())
    );
    assert_eq!(
        may_delegate(&go, PlatformRole::AppOperator, &Scope::All),
        Ok(())
    );
    assert_eq!(
        may_delegate(&go, PlatformRole::AppOperator, &Scope::Orgs(vec![org()])),
        Ok(())
    );
}

#[test]
fn unbounded_admin_may_issue_app_operators_at_any_scope() {
    let ga = admin_over(Scope::All);
    assert_eq!(
        may_delegate(&ga, PlatformRole::AppOperator, &Scope::All),
        Ok(())
    );
    assert_eq!(
        may_delegate(&ga, PlatformRole::AppOperator, &Scope::Orgs(vec![org()])),
        Ok(())
    );
}

#[test]
fn admin_may_not_mint_or_touch_a_peer() {
    let ga = admin_over(Scope::All);
    // Pins the `>=` in the rank comparison. A `>` would let a Global Admin create
    // another Global Admin, and — since delete reads the target row — delete one.
    // That is peer minting and peer removal, and it is the Owner's call.
    assert_eq!(
        may_delegate(&ga, PlatformRole::GlobalAdmin, &Scope::All),
        Err(DelegationDenial::RoleNotBelow)
    );
    assert_eq!(
        may_delegate(&ga, PlatformRole::GlobalAdmin, &Scope::Orgs(vec![org()])),
        Err(DelegationDenial::RoleNotBelow),
        "narrowing the scope must not buy a peer-tier grant"
    );
}

#[test]
fn self_edit_is_refused_structurally() {
    // The actor IS a `global_admin`, so their own row's role is `global_admin`,
    // which is never strictly below itself. No identity comparison is involved
    // and none is needed — this is why the rule is one sentence and not three.
    let bounded = admin_over(Scope::Orgs(vec![org()]));
    assert_eq!(
        may_delegate(&bounded, PlatformRole::GlobalAdmin, &Scope::All),
        Err(DelegationDenial::RoleNotBelow),
        "a bounded admin widening their own row to scope_all is THE escalation \
         this bound exists to stop"
    );
}

#[test]
fn bounded_admin_may_not_issue_beyond_its_own_orgs() {
    let a = org();
    let b = Uuid::from_u128(0xBEEF);
    let ga = admin_over(Scope::Orgs(vec![a]));

    assert_eq!(
        may_delegate(&ga, PlatformRole::AppOperator, &Scope::Orgs(vec![a])),
        Ok(())
    );
    // Pins `Scope::contains`'s (Orgs, All) => false arm. Without it a bounded
    // operator launders itself unbounded through a second account.
    assert_eq!(
        may_delegate(&ga, PlatformRole::AppOperator, &Scope::All),
        Err(DelegationDenial::ScopeNotContained)
    );
    // Pins the subset test rather than mere non-emptiness.
    assert_eq!(
        may_delegate(&ga, PlatformRole::AppOperator, &Scope::Orgs(vec![b])),
        Err(DelegationDenial::ScopeNotContained)
    );
    assert_eq!(
        may_delegate(&ga, PlatformRole::AppOperator, &Scope::Orgs(vec![a, b])),
        Err(DelegationDenial::ScopeNotContained),
        "a superset containing one permitted org is still a superset"
    );
}

#[test]
fn app_operator_and_non_staff_may_not_delegate_at_all() {
    let op = PrincipalFacts {
        platform: Some(PlatformStanding::from_role(
            PlatformRole::AppOperator,
            Scope::All,
        )),
        ..facts()
    };
    // Capability, not rank, is what stops this — an App Operator has no
    // `ManagePlatformGrants`, so it fails before any comparison.
    assert_eq!(
        may_delegate(&op, PlatformRole::AppOperator, &Scope::All),
        Err(DelegationDenial::NoCapability)
    );
    assert_eq!(
        may_delegate(&facts(), PlatformRole::AppOperator, &Scope::All),
        Err(DelegationDenial::NotStaff)
    );
}

#[test]
fn scope_contains_is_a_subset_test_not_an_overlap_test() {
    let a = org();
    let b = Uuid::from_u128(0xBEEF);
    assert!(Scope::All.contains(&Scope::All));
    assert!(Scope::All.contains(&Scope::Orgs(vec![a])));
    assert!(!Scope::Orgs(vec![a]).contains(&Scope::All));
    assert!(Scope::Orgs(vec![a, b]).contains(&Scope::Orgs(vec![a])));
    assert!(!Scope::Orgs(vec![a]).contains(&Scope::Orgs(vec![a, b])));
    // An empty target reaches nothing, so every scope contains it. Harmless, and
    // stated so a reader does not mistake it for a hole.
    assert!(Scope::Orgs(vec![]).contains(&Scope::Orgs(vec![])));
}

#[test]
fn manage_platform_grants_is_an_admin_capability_only() {
    assert!(
        PlatformRole::GlobalAdmin
            .caps()
            .contains(&Cap::ManagePlatformGrants)
    );
    assert!(
        !PlatformRole::AppOperator
            .caps()
            .contains(&Cap::ManagePlatformGrants)
    );
    // The door and the fence are different questions; both must exist. Asserted
    // through `allows` rather than by comparing rings: `Ring` is private on purpose
    // (a public ring lets a call site pick its own authority level), and asserting
    // the decision is the stronger claim anyway.
    let p = Resource::platform();
    let ga = PrincipalFacts {
        platform: global_admin_standing(),
        ..facts()
    };
    let op = PrincipalFacts {
        platform: Some(PlatformStanding::from_role(
            PlatformRole::AppOperator,
            Scope::All,
        )),
        ..facts()
    };
    assert!(allows(&ga, Action::PlatformGrants, &p));
    assert!(
        !allows(&op, Action::PlatformGrants, &p),
        "an App Operator must not reach the grant console at all"
    );
    assert!(
        !allows(&facts(), Action::PlatformGrants, &p),
        "a non-staff principal must not reach the grant console"
    );
}
