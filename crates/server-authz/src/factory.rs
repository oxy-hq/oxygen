//! May this caller use the Factory in this workspace?
//!
//! **The one decision.** Oxy staff over the workspace's org, an owner or admin of that
//! org, or a member of that org an Oxy operator granted this workspace to. Everything
//! that asks — the server-side gate on every Factory route (`oxy-app`'s `server::factory_access`), and the flag the
//! workspace response carries for the rail — goes through [`may_use_factory`], so the
//! two cannot disagree about who is let in.
//!
//! The rule itself is `Action::FactoryUse` / `Ring::Factory` in `oxy-authz`. This is
//! the call that names it for a workspace, and nothing more: it adds no term of its
//! own, and an empty fact set is refused like any other decision.

use oxy_authz::{Action, PrincipalFacts, Resource, allows};
use uuid::Uuid;

/// Whether `facts` may use the Factory in the workspace `workspace_id`, which belongs
/// to the org `org_id`.
pub fn may_use_factory(facts: &PrincipalFacts, workspace_id: Uuid, org_id: Uuid) -> bool {
    allows(
        facts,
        Action::FactoryUse,
        &Resource::workspace(workspace_id, org_id),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy_authz::{PlatformRole, PlatformStanding, Scope};

    const ORG: Uuid = Uuid::from_u128(1);
    const WS: Uuid = Uuid::from_u128(2);

    #[test]
    fn staff_an_org_officer_and_a_granted_member_are_let_in_and_nobody_else() {
        let staff = PrincipalFacts {
            platform: Some(PlatformStanding::from_role(
                PlatformRole::GlobalAdmin,
                Scope::All,
            )),
            ..Default::default()
        };
        let granted = PrincipalFacts {
            member_orgs: vec![ORG],
            factory_workspaces: vec![WS],
            ..Default::default()
        };
        let owner_without_grant = PrincipalFacts {
            owned_orgs: vec![ORG],
            admin_orgs: vec![ORG],
            member_orgs: vec![ORG],
            ..Default::default()
        };
        let admin_without_grant = PrincipalFacts {
            admin_orgs: vec![ORG],
            member_orgs: vec![ORG],
            ..Default::default()
        };
        let member_without_grant = PrincipalFacts {
            member_orgs: vec![ORG],
            ..Default::default()
        };
        assert!(may_use_factory(&staff, WS, ORG));
        assert!(may_use_factory(&granted, WS, ORG));
        assert!(may_use_factory(&owner_without_grant, WS, ORG));
        assert!(may_use_factory(&admin_without_grant, WS, ORG));
        assert!(!may_use_factory(&member_without_grant, WS, ORG));
        assert!(!may_use_factory(&PrincipalFacts::default(), WS, ORG));
    }

    #[test]
    fn a_grant_for_another_workspace_opens_nothing_here() {
        let granted_elsewhere = PrincipalFacts {
            member_orgs: vec![ORG],
            factory_workspaces: vec![Uuid::from_u128(3)],
            ..Default::default()
        };
        assert!(!may_use_factory(&granted_elsewhere, WS, ORG));
    }
}
