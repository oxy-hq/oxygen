//! A service account in the model (API-tokens design §3.3).
//!
//! It stands in its one org as a member or an admin, from its own fact rather
//! than from `org_members`. So the proof is differential: a service account
//! holding a token decides exactly as a **person** of the same role holding
//! the same token — with three deliberate exceptions, each asserted here:
//! it is never an owner, it does not bill, and it holds no platform or
//! partner standing whatever else its facts say.

use uuid::Uuid;

use crate::{
    Action, Cap, PartnerStanding, PlatformRole, PlatformStanding, PrincipalFacts, Resource,
    RoleCeiling, Scope, ServiceAccountStanding, TokenGrant, TokenReach, allows,
};

const ORG: Uuid = Uuid::from_u128(0xA);
const OTHER_ORG: Uuid = Uuid::from_u128(0xB);
const WS_A: Uuid = Uuid::from_u128(0xA1);
const WS_B: Uuid = Uuid::from_u128(0xA2);
const APP: Uuid = Uuid::from_u128(0xAA);
const USER: Uuid = Uuid::from_u128(0x1);

/// A service account of `ORG`, before its token narrows it.
fn account(admin: bool) -> PrincipalFacts {
    PrincipalFacts {
        user_id: USER,
        service_account: Some(ServiceAccountStanding { org_id: ORG, admin }),
        ..Default::default()
    }
}

/// A person of `role` in `ORG`.
fn person(role: RoleCeiling) -> PrincipalFacts {
    let set = |on: bool| if on { vec![ORG] } else { Vec::new() };
    PrincipalFacts {
        user_id: USER,
        member_orgs: vec![ORG],
        admin_orgs: set(role >= RoleCeiling::Admin),
        owned_orgs: set(role >= RoleCeiling::Owner),
        ..Default::default()
    }
}

/// A service-account token: grant-bound, never all-access, no standing flags.
fn token(grants: Vec<TokenGrant>) -> TokenReach {
    TokenReach {
        all_access: false,
        platform: false,
        partner: false,
        grants,
        blocked_orgs: Vec::new(),
        sandbox_agent: None,
    }
}

fn grant(org_id: Uuid, workspace_id: Option<Uuid>, ceiling: RoleCeiling) -> TokenGrant {
    TokenGrant {
        org_id,
        workspace_id,
        ceiling,
    }
}

fn org_wide(ceiling: RoleCeiling) -> TokenReach {
    token(vec![grant(ORG, None, ceiling)])
}

fn resources_in(org: Uuid) -> Vec<Resource> {
    vec![
        Resource::org(org),
        Resource::workspace(WS_A, org),
        Resource::workspace_with_creator(WS_A, org, Some(USER)),
        Resource::app(APP, org),
        Resource::app_with_visibility(APP, org, true),
        Resource::platform(),
    ]
}

/// `account` with `token` decides as `person` with the same token, for every
/// action but the ones in `except`, which it is denied.
fn assert_decides_like(
    account: &PrincipalFacts,
    person: &PrincipalFacts,
    token: &TokenReach,
    except: &[Action],
) {
    let account = account.clone().narrowed_by(token);
    let person = person.clone().narrowed_by(token);
    for resource in resources_in(ORG) {
        for action in Action::ALL {
            let got = allows(&account, action, &resource);
            if except.contains(&action) {
                assert!(!got, "{action:?} on {:?} must be denied", resource.kind);
            } else {
                let want = allows(&person, action, &resource);
                assert_eq!(got, want, "{action:?} on {:?}", resource.kind);
            }
        }
    }
}

#[test]
fn a_member_account_decides_as_a_member_does() {
    let member = person(RoleCeiling::Member);
    for ceiling in RoleCeiling::ALL {
        assert_decides_like(&account(false), &member, &org_wide(ceiling), &[]);
    }
    // And it is not a sweep of denials: it reads the org and edits a workspace.
    let facts = account(false).narrowed_by(&org_wide(RoleCeiling::Member));
    assert!(allows(&facts, Action::OrgRead, &Resource::org(ORG)));
    assert!(allows(
        &facts,
        Action::WorkspaceEdit,
        &Resource::workspace(WS_A, ORG)
    ));
    assert!(!allows(
        &facts,
        Action::ServiceAccountManage,
        &Resource::org(ORG)
    ));
}

#[test]
fn an_admin_account_decides_as_an_admin_does_but_does_not_bill() {
    let admin = person(RoleCeiling::Admin);
    for ceiling in RoleCeiling::ALL {
        assert_decides_like(
            &account(true),
            &admin,
            &org_wide(ceiling),
            &[Action::OrgBilling],
        );
    }
    let facts = account(true).narrowed_by(&org_wide(RoleCeiling::Admin));
    assert!(allows(&facts, Action::MemberSetRole, &Resource::org(ORG)));
    assert!(allows(
        &facts,
        Action::WorkspaceManage,
        &Resource::workspace(WS_A, ORG)
    ));
    // The person it is compared with does bill — the exception is real.
    let person = admin.narrowed_by(&org_wide(RoleCeiling::Admin));
    assert!(allows(&person, Action::OrgBilling, &Resource::org(ORG)));
}

#[test]
fn an_account_is_never_an_owner() {
    // `owner` is the widest ceiling a grant can name, and an admin the highest
    // an account can be: nothing owner-only is ever reachable.
    let facts = account(true).narrowed_by(&org_wide(RoleCeiling::Owner));
    assert!(!allows(&facts, Action::OrgOwnerManage, &Resource::org(ORG)));
}

#[test]
fn the_accounts_role_caps_what_a_grant_can_give() {
    // A member account whose grant says `admin` is still a member.
    let facts = account(false).narrowed_by(&org_wide(RoleCeiling::Admin));
    assert!(!allows(&facts, Action::MemberSetRole, &Resource::org(ORG)));
    assert!(!allows(
        &facts,
        Action::WorkspaceManage,
        &Resource::workspace(WS_A, ORG)
    ));
    // And an admin account whose grant says `member` acts as a member.
    let facts = account(true).narrowed_by(&org_wide(RoleCeiling::Member));
    assert!(!allows(&facts, Action::MemberSetRole, &Resource::org(ORG)));
    assert!(allows(
        &facts,
        Action::WorkspaceEdit,
        &Resource::workspace(WS_A, ORG)
    ));
}

#[test]
fn an_account_reaches_nothing_outside_its_grants_or_its_org() {
    // A grant on one workspace: that workspace, not its sibling, not the org.
    let one = token(vec![grant(ORG, Some(WS_A), RoleCeiling::Admin)]);
    let facts = account(true).narrowed_by(&one);
    assert!(allows(
        &facts,
        Action::WorkspaceEdit,
        &Resource::workspace(WS_A, ORG)
    ));
    assert!(!allows(
        &facts,
        Action::WorkspaceEdit,
        &Resource::workspace(WS_B, ORG)
    ));
    assert!(!allows(&facts, Action::OrgRead, &Resource::org(ORG)));

    // Another org: nothing, even if a grant row named it.
    let stray = token(vec![grant(OTHER_ORG, None, RoleCeiling::Owner)]);
    let facts = account(true).narrowed_by(&stray);
    // (The creator rule is keyed by the resource's owner, not by standing, and
    // an account creates nothing outside its org — so no owned resource here.)
    for resource in resources_in(OTHER_ORG)
        .into_iter()
        .filter(|r| r.owner.is_none())
    {
        for action in Action::ALL {
            assert!(
                !allows(&facts, action, &resource),
                "{action:?} on {:?}",
                resource.kind
            );
        }
    }
}

/// Facts that say the principal is root, staff and a partner — the rows a
/// service account must not be able to use even if someone adds them.
fn with_every_standing(mut facts: PrincipalFacts) -> PrincipalFacts {
    facts.is_global_owner = true;
    facts.platform = Some(PlatformStanding::from_role(
        PlatformRole::GlobalAdmin,
        Scope::All,
    ));
    facts.partners = vec![PartnerStanding {
        partner_id: Uuid::from_u128(0xC),
        client_orgs: vec![OTHER_ORG],
        caps: vec![Cap::ManageMembers, Cap::DevelopApps, Cap::ManageApps],
    }];
    facts
}

#[test]
fn an_account_holds_no_platform_or_partner_standing_by_construction() {
    // Every standing in the facts, and a token that claims to carry them all.
    let carries_all = TokenReach {
        all_access: true,
        platform: true,
        partner: true,
        grants: Vec::new(),
        blocked_orgs: Vec::new(),
        sandbox_agent: None,
    };
    for facts in [
        with_every_standing(account(true)),
        with_every_standing(account(true)).narrowed_by(&carries_all),
        with_every_standing(account(true)).narrowed_by(&org_wide(RoleCeiling::Owner)),
    ] {
        assert!(!facts.is_staff() && !facts.is_root() && !facts.is_partner());
        assert!(facts.platform_scope().is_none());
        assert!(!facts.manages(OTHER_ORG));
        for action in Action::ALL {
            assert!(
                !allows(&facts, action, &Resource::platform()),
                "{action:?} on the platform"
            );
            // Standing reaches no other tenant either.
            assert!(
                !allows(&facts, action, &Resource::org(OTHER_ORG)),
                "{action:?} on another org"
            );
        }
    }
    // The same facts without the account fact do hold standing: the denial
    // above is the account's, not an accident of the fixture.
    let person = with_every_standing(PrincipalFacts {
        user_id: USER,
        ..Default::default()
    });
    assert!(person.is_staff() && person.is_root() && person.is_partner());
}
