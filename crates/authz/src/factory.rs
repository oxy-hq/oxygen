//! May this caller use the Factory in this workspace?
//!
//! **The one decision.** Oxy staff over the workspace's org, an owner or admin of that
//! org, or a member of that org an Oxy operator granted this workspace to. Everything
//! that asks — the server-side gate on every Factory route (`oxy-app`'s `server::factory_access`), and the flag the
//! workspace response carries for the rail — goes through [`may_use_factory`], so the
//! two cannot disagree about who is let in.
//!
//! The rule itself is `Action::FactoryUse` / `Ring::Factory` in this crate. This is
//! the call that names it for a workspace, and nothing more: it adds no term of its
//! own, and an empty fact set is refused like any other decision.
//!
//! Moved here from `oxy-server-authz` (C3a, 2026-11-28): the checks are pure over
//! facts, so `oxy-workspace-runtime`'s Factory gate can name them without the
//! enforcement crate. `oxy_server_authz::factory` re-exports this module.

use uuid::Uuid;

use crate::{Action, PrincipalFacts, Resource, allows};

/// Whether `facts` may use the Factory in the workspace `workspace_id`, which belongs
/// to the org `org_id`.
pub fn may_use_factory(facts: &PrincipalFacts, workspace_id: Uuid, org_id: Uuid) -> bool {
    allows(
        facts,
        Action::FactoryUse,
        &Resource::workspace(workspace_id, org_id),
    )
}

/// Whether `facts` may use the Factory in SOME workspace of `org_id` — the ring's staff
/// and officer terms, which name no workspace. A grant is per workspace, so a grantee is
/// admitted by [`may_use_factory`] on that workspace, not by this.
///
/// `Resource::org` makes the grant term false by construction (the ring reads it only
/// against a workspace resource), which is exactly the quantification wanted: *could
/// this person open Nexus somewhere in this org by standing alone*. Asked by the
/// notification catalogue, which segments by operator standing and knows an org rather
/// than a workspace.
pub fn may_use_factory_in_org(facts: &PrincipalFacts, org_id: Uuid) -> bool {
    allows(facts, Action::FactoryUse, &Resource::org(org_id))
}

/// Whether `facts` may **check a compile** of the workspace — queue a staging compile
/// of a branch and read its status — which is the one thing the Factory gate lets
/// through for a caller [`may_use_factory`] refuses, on exactly the two routes that do
/// it (`server::factory_access::routes::COMPILE_CHECK`).
///
/// `Action::CompileCheck`: for a person it is `FactoryUse` AND `WorkspaceEdit`, so it
/// admits no person the Factory refuses; the one caller it adds is the org's service
/// account acting through a CI run token, at a `viewer` ceiling — the customer CI
/// compile gate.
pub fn may_check_compile(facts: &PrincipalFacts, workspace_id: Uuid, org_id: Uuid) -> bool {
    allows(
        facts,
        Action::CompileCheck,
        &Resource::workspace(workspace_id, org_id),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        PlatformRole, PlatformStanding, RoleCeiling, Scope, ServiceAccountStanding, TokenGrant,
        TokenReach,
    };

    /// The org's service account through a CI run token holding `viewer` on `WS`.
    fn ci_account(ci_run: bool) -> PrincipalFacts {
        PrincipalFacts {
            service_account: Some(ServiceAccountStanding {
                org_id: ORG,
                admin: false,
            }),
            ..Default::default()
        }
        .narrowed_by(&TokenReach {
            all_access: false,
            platform: false,
            partner: false,
            grants: vec![TokenGrant {
                org_id: ORG,
                workspace_id: Some(WS),
                ceiling: RoleCeiling::Viewer,
            }],
            blocked_orgs: Vec::new(),
            sandbox_agent: None,
            app: None,
            ci_run,
        })
    }

    #[test]
    fn the_ci_account_checks_a_compile_but_does_not_use_the_factory() {
        assert!(may_check_compile(&ci_account(true), WS, ORG));
        assert!(!may_use_factory(&ci_account(true), WS, ORG));
        // Its own long-lived token is not a CI run.
        assert!(!may_check_compile(&ci_account(false), WS, ORG));
        assert!(!may_check_compile(
            &ci_account(true),
            Uuid::from_u128(3),
            ORG
        ));
    }

    #[test]
    fn no_person_checks_a_compile_whom_the_factory_refuses() {
        let member_without_grant = PrincipalFacts {
            member_orgs: vec![ORG],
            ..Default::default()
        };
        let granted = PrincipalFacts {
            member_orgs: vec![ORG],
            factory_workspaces: vec![WS],
            ..Default::default()
        };
        assert!(!may_check_compile(&member_without_grant, WS, ORG));
        assert!(may_check_compile(&granted, WS, ORG));
        assert!(!may_check_compile(&PrincipalFacts::default(), WS, ORG));
    }

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
    fn standing_in_an_org_is_the_ring_without_its_grant_term() {
        let granted = PrincipalFacts {
            member_orgs: vec![ORG],
            factory_workspaces: vec![WS],
            ..Default::default()
        };
        let admin = PrincipalFacts {
            admin_orgs: vec![ORG],
            member_orgs: vec![ORG],
            ..Default::default()
        };
        let staff = PrincipalFacts {
            platform: Some(PlatformStanding::from_role(
                PlatformRole::GlobalAdmin,
                Scope::All,
            )),
            ..Default::default()
        };
        assert!(may_use_factory_in_org(&admin, ORG));
        assert!(may_use_factory_in_org(&staff, ORG));
        // A grantee holds the Factory in a workspace, not a standing in the org: the
        // grant term reads a workspace resource and an org names none.
        assert!(may_use_factory(&granted, WS, ORG));
        assert!(!may_use_factory_in_org(&granted, ORG));
        assert!(!may_use_factory_in_org(&PrincipalFacts::default(), ORG));
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
