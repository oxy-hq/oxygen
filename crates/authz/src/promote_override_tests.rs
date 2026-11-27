//! `Action::AppPromoteOverride`: who may promote a build past its promote
//! checks. An owner or admin of the app's org, or Oxy staff reaching in with
//! `manage_apps`; never the org's service account, a CI run token, an
//! `app_members` admin, a partner, a member, or a sandbox agent token.

use uuid::Uuid;

use crate::{
    Action, Cap, PartnerStanding, PlatformRole, PlatformStanding, PrincipalFacts, Resource,
    RoleCeiling, SandboxAgentReach, SandboxApp, Scope, ServiceAccountStanding, TokenGrant,
    TokenReach, allows,
};

fn org() -> Uuid {
    Uuid::from_u128(0xa2)
}

fn other_org() -> Uuid {
    Uuid::from_u128(0xa3)
}

fn app() -> Uuid {
    Uuid::from_u128(0xb2)
}

fn workspace() -> Uuid {
    Uuid::from_u128(0xd2)
}

fn the_app() -> Resource {
    Resource::app(app(), org()).published_from(workspace())
}

fn person() -> PrincipalFacts {
    PrincipalFacts {
        user_id: Uuid::from_u128(0xc2),
        ..PrincipalFacts::default()
    }
}

fn owner() -> PrincipalFacts {
    PrincipalFacts {
        owned_orgs: vec![org()],
        admin_orgs: vec![org()],
        member_orgs: vec![org()],
        ..person()
    }
}

fn admin() -> PrincipalFacts {
    PrincipalFacts {
        admin_orgs: vec![org()],
        member_orgs: vec![org()],
        ..person()
    }
}

fn staff(role: PlatformRole, scope: Scope) -> PrincipalFacts {
    PrincipalFacts {
        platform: Some(PlatformStanding::from_role(role, scope)),
        ..person()
    }
}

/// A token covering the app's workspace at `ceiling`, both standings carried.
fn token(ceiling: RoleCeiling) -> TokenReach {
    TokenReach {
        all_access: false,
        grants: vec![TokenGrant {
            org_id: org(),
            workspace_id: Some(workspace()),
            ceiling,
        }],
        ..TokenReach::unrestricted()
    }
}

fn may(facts: &PrincipalFacts) -> bool {
    allows(facts, Action::AppPromoteOverride, &the_app())
}

#[test]
fn an_owner_or_admin_of_the_apps_org_may_override() {
    assert!(may(&owner()));
    assert!(may(&admin()));
}

#[test]
fn nobody_below_an_officer_may_override() {
    let member = PrincipalFacts {
        member_orgs: vec![org()],
        ..person()
    };
    let app_admin = PrincipalFacts {
        app_memberships: vec![app()],
        app_admin_memberships: vec![app()],
        ..member.clone()
    };
    let elsewhere = PrincipalFacts {
        owned_orgs: vec![other_org()],
        admin_orgs: vec![other_org()],
        member_orgs: vec![other_org()],
        ..person()
    };
    for (who, facts) in [
        ("a plain member", member),
        ("the app's own admin", app_admin),
        ("an owner of another org", elsewhere),
        ("nobody", person()),
    ] {
        assert!(!may(&facts), "{who} overrode the checks");
    }
}

#[test]
fn staff_override_only_with_manage_apps_reaching_the_org() {
    assert!(may(&staff(PlatformRole::GlobalAdmin, Scope::All)));
    assert!(may(&staff(
        PlatformRole::AppOperator,
        Scope::Orgs(vec![org()])
    )));
    let root = PrincipalFacts {
        is_global_owner: true,
        ..person()
    };
    assert!(may(&root));

    let out_of_scope = staff(PlatformRole::AppOperator, Scope::Orgs(vec![other_org()]));
    assert!(!may(&out_of_scope), "a grant that does not reach the org");
    let develop_only = PrincipalFacts {
        platform: Some(PlatformStanding {
            role: PlatformRole::AppOperator,
            caps: vec![Cap::DevelopApps],
            scope: Scope::All,
        }),
        ..person()
    };
    assert!(!may(&develop_only), "develop_apps is not manage_apps");
    assert!(!may(&staff(PlatformRole::AppChecker, Scope::All)));
}

/// The staff term is the operator override: a staff member who is a plain
/// member of the org is a plain member there, and an officer there by their
/// own role.
#[test]
fn staff_standing_does_not_out_rank_a_real_membership() {
    let plain_member = PrincipalFacts {
        member_orgs: vec![org()],
        ..staff(PlatformRole::GlobalAdmin, Scope::All)
    };
    assert!(!may(&plain_member));
    let officer = PrincipalFacts {
        admin_orgs: vec![org()],
        member_orgs: vec![org()],
        ..staff(PlatformRole::GlobalAdmin, Scope::All)
    };
    assert!(may(&officer));
}

#[test]
fn a_service_account_never_overrides_and_so_neither_does_a_ci_run_token() {
    let account = |admin: bool| PrincipalFacts {
        service_account: Some(ServiceAccountStanding {
            org_id: org(),
            admin,
        }),
        ..person()
    };
    assert!(!may(&account(false)));
    assert!(!may(&account(true)), "an admin service account");
    let ci_run = PrincipalFacts {
        token: Some(TokenReach {
            ci_run: true,
            ..token(RoleCeiling::Admin)
        }),
        ..account(true)
    };
    assert!(!may(&ci_run), "a CI run token acts as the account");
    // Staff standing on the same facts is not carried by an account either.
    let with_grant = PrincipalFacts {
        platform: Some(PlatformStanding::from_role(
            PlatformRole::GlobalAdmin,
            Scope::All,
        )),
        is_global_owner: true,
        ..account(true)
    };
    assert!(!may(&with_grant));
}

#[test]
fn a_partner_never_overrides() {
    let partner = PrincipalFacts {
        partners: vec![PartnerStanding {
            partner_id: Uuid::from_u128(0xe2),
            client_orgs: vec![org()],
            caps: vec![Cap::ManageApps, Cap::DevelopApps, Cap::ManageMembers],
        }],
        ..person()
    };
    assert!(!may(&partner));
}

#[test]
fn a_token_carries_it_only_at_an_admin_ceiling_and_with_its_standing() {
    for (ceiling, carried) in [
        (RoleCeiling::Viewer, false),
        (RoleCeiling::Member, false),
        (RoleCeiling::Admin, true),
        (RoleCeiling::Owner, true),
    ] {
        let officer = PrincipalFacts {
            token: Some(token(ceiling)),
            ..owner()
        };
        assert_eq!(may(&officer), carried, "an owner's token at {ceiling:?}");
        let operator = PrincipalFacts {
            token: Some(token(ceiling)),
            ..staff(PlatformRole::AppOperator, Scope::All)
        };
        assert_eq!(may(&operator), carried, "staff's token at {ceiling:?}");
    }
    // A token that does not carry platform standing holds no staff term.
    let no_platform = PrincipalFacts {
        token: Some(TokenReach {
            platform: false,
            ..token(RoleCeiling::Admin)
        }),
        ..staff(PlatformRole::GlobalAdmin, Scope::All)
    };
    assert!(!may(&no_platform));
    // A token whose grants do not reach the app's workspace covers nothing.
    let elsewhere = PrincipalFacts {
        token: Some(TokenReach {
            all_access: false,
            grants: vec![TokenGrant {
                org_id: org(),
                workspace_id: Some(Uuid::from_u128(0xd9)),
                ceiling: RoleCeiling::Owner,
            }],
            ..TokenReach::unrestricted()
        }),
        ..owner()
    };
    assert!(!may(&elsewhere));
}

#[test]
fn a_sandbox_agent_token_never_overrides() {
    let agent = PrincipalFacts {
        token: Some(TokenReach {
            sandbox_agent: Some(SandboxAgentReach {
                token_id: Uuid::from_u128(0xf2),
                apps: vec![SandboxApp {
                    app_id: app(),
                    org_id: org(),
                    staging: true,
                }],
            }),
            ..token(RoleCeiling::Admin)
        }),
        ..staff(PlatformRole::GlobalAdmin, Scope::All)
    };
    assert!(!may(&agent));
}

/// The decision is about one app: an org, a workspace or the platform is not
/// a thing whose checks can be overridden, whoever asks.
#[test]
fn only_an_app_resource_is_decided() {
    for resource in [
        Resource::org(org()),
        Resource::workspace(workspace(), org()),
        Resource::platform(),
    ] {
        for facts in [owner(), staff(PlatformRole::GlobalAdmin, Scope::All)] {
            assert!(
                !allows(&facts, Action::AppPromoteOverride, &resource),
                "{:?}",
                resource.kind
            );
        }
    }
    // Another org's app, for this org's owner.
    let theirs = Resource::app(app(), other_org());
    assert!(!allows(&owner(), Action::AppPromoteOverride, &theirs));
}
