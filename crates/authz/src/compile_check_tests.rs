//! [`Action::CompileCheck`]: the customer CI compile gate (open question 2 of
//! `internal-docs/oxy-cli-to-oxyc.md`, decided 2026-11-21 as a narrow action,
//! not editor standing for the org's service account).
//!
//! Two properties, each proved over a sweep rather than a sample:
//!
//! - **unchanged for every person**: the action is exactly `FactoryUse AND
//!   WorkspaceEdit`, what the two staging compile routes asked of a person before
//!   the action existed (the Factory gate, then `WorkspaceEditor`);
//! - **the CI account gains this and nothing else**: the org's service account
//!   through a CI run token (`TokenReach::ci_run`) at a `viewer` ceiling may check
//!   a compile of the workspace its grant names, and decides every OTHER action
//!   exactly as the same account through a token without the fact.

use uuid::Uuid;

use crate::{
    Action, Cap, PlatformRole, PlatformStanding, PrincipalFacts, Resource, RoleCeiling, Scope,
    ServiceAccountStanding, TokenGrant, TokenReach, allows,
};

const ORG: Uuid = Uuid::from_u128(0xA);
const OTHER_ORG: Uuid = Uuid::from_u128(0xB);
const WS: Uuid = Uuid::from_u128(0xA1);
const OTHER_WS: Uuid = Uuid::from_u128(0xA2);
const APP: Uuid = Uuid::from_u128(0xAA);
const USER: Uuid = Uuid::from_u128(0x1);

fn grant(workspace_id: Option<Uuid>, ceiling: RoleCeiling) -> TokenGrant {
    TokenGrant {
        org_id: ORG,
        workspace_id,
        ceiling,
    }
}

/// A service-account-shaped token: grant-bound, no standing flags.
fn token(grants: Vec<TokenGrant>, ci_run: bool) -> TokenReach {
    TokenReach {
        all_access: false,
        platform: false,
        partner: false,
        grants,
        blocked_orgs: Vec::new(),
        sandbox_agent: None,
        app: None,
        ci_run,
    }
}

/// What a trust policy for the CI gate hands its run: one workspace, `viewer`.
fn ci_viewer() -> TokenReach {
    token(vec![grant(Some(WS), RoleCeiling::Viewer)], true)
}

fn account_of(org_id: Uuid, admin: bool) -> PrincipalFacts {
    PrincipalFacts {
        user_id: USER,
        service_account: Some(ServiceAccountStanding { org_id, admin }),
        ..Default::default()
    }
}

/// Every resource shape a decision can name, in and out of the org.
fn resources() -> Vec<Resource> {
    vec![
        Resource::org(ORG),
        Resource::workspace(WS, ORG),
        Resource::workspace_with_creator(WS, ORG, Some(USER)),
        Resource::workspace(OTHER_WS, ORG),
        Resource::app(APP, ORG),
        Resource::app(APP, ORG).published_from(WS),
        Resource::workspace(Uuid::from_u128(0xB1), OTHER_ORG),
        Resource::platform(),
    ]
}

#[test]
fn the_orgs_account_through_a_ci_run_token_checks_a_compile_of_its_workspace_at_viewer() {
    for admin in [false, true] {
        let facts = account_of(ORG, admin).narrowed_by(&ci_viewer());
        let ws = Resource::workspace(WS, ORG);
        assert!(allows(&facts, Action::CompileCheck, &ws), "admin={admin}");
        // ...and it is still a viewer: it does not edit the workspace, use the
        // Factory, or check a workspace its grant does not name.
        assert!(!allows(&facts, Action::WorkspaceEdit, &ws), "admin={admin}");
        assert!(!allows(&facts, Action::FactoryUse, &ws), "admin={admin}");
        assert!(!allows(
            &facts,
            Action::CompileCheck,
            &Resource::workspace(OTHER_WS, ORG)
        ));
        assert!(!allows(&facts, Action::CompileCheck, &Resource::org(ORG)));
        assert!(!allows(&facts, Action::CompileCheck, &Resource::platform()));
    }
}

/// The proof that the fact widens exactly one action: for every ceiling, every
/// grant shape and every resource, the CI account decides every other action
/// precisely as the same account through a token without the fact.
#[test]
fn the_ci_run_fact_moves_no_decision_but_the_compile_check() {
    for admin in [false, true] {
        for ceiling in RoleCeiling::ALL {
            for workspace_id in [None, Some(WS)] {
                let grants = vec![grant(workspace_id, ceiling)];
                let with = account_of(ORG, admin).narrowed_by(&token(grants.clone(), true));
                let without = account_of(ORG, admin).narrowed_by(&token(grants, false));
                for action in Action::ALL {
                    if action == Action::CompileCheck {
                        continue;
                    }
                    for resource in resources() {
                        assert_eq!(
                            allows(&with, action, &resource),
                            allows(&without, action, &resource),
                            "admin={admin} {ceiling:?} {workspace_id:?} {action:?} on {resource:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn the_same_account_through_its_own_long_lived_token_is_refused_at_viewer() {
    // `oxy_sat_`: the account's standing, no CI run. A viewer grant does not
    // let it check a compile — the narrow door is a CI run's.
    let facts = account_of(ORG, false)
        .narrowed_by(&token(vec![grant(Some(WS), RoleCeiling::Viewer)], false));
    assert!(!allows(
        &facts,
        Action::CompileCheck,
        &Resource::workspace(WS, ORG)
    ));
}

#[test]
fn another_orgs_account_is_refused_whatever_its_token_says() {
    // A token naming ORG on an account of OTHER_ORG: admission refuses such a
    // row, and the model does not rely on it having done so.
    let facts = account_of(OTHER_ORG, true).narrowed_by(&ci_viewer());
    assert!(!allows(
        &facts,
        Action::CompileCheck,
        &Resource::workspace(WS, ORG)
    ));
}

#[test]
fn a_person_holding_the_fact_by_mistake_gains_nothing() {
    // A viewer-ceiling person token wrongly marked as a CI run: the account
    // term needs the org's service account, so the person term decides — and
    // a viewer neither uses the Factory nor edits.
    let person = PrincipalFacts {
        user_id: USER,
        member_orgs: vec![ORG],
        admin_orgs: vec![ORG],
        ..Default::default()
    };
    let facts = person.narrowed_by(&token(vec![grant(Some(WS), RoleCeiling::Viewer)], true));
    assert!(!allows(
        &facts,
        Action::CompileCheck,
        &Resource::workspace(WS, ORG)
    ));
}

/// Every person shape the Factory and the editor guard distinguish, each at
/// every ceiling and with no token at all.
fn people() -> Vec<(&'static str, PrincipalFacts)> {
    let base = || PrincipalFacts {
        user_id: USER,
        ..Default::default()
    };
    let operator = PlatformStanding::from_role(PlatformRole::GlobalAdmin, Scope::All);
    let app_operator = PlatformStanding::from_role(PlatformRole::AppOperator, Scope::All);
    vec![
        ("nobody", base()),
        (
            "plain member",
            PrincipalFacts {
                member_orgs: vec![ORG],
                ..base()
            },
        ),
        (
            "member with the workspace's factory grant",
            PrincipalFacts {
                member_orgs: vec![ORG],
                factory_workspaces: vec![WS],
                ..base()
            },
        ),
        (
            "member with another workspace's factory grant",
            PrincipalFacts {
                member_orgs: vec![ORG],
                factory_workspaces: vec![OTHER_WS],
                ..base()
            },
        ),
        (
            "member elevated to workspace admin",
            PrincipalFacts {
                member_orgs: vec![ORG],
                ws_admin_override: vec![WS],
                ..base()
            },
        ),
        (
            "org admin",
            PrincipalFacts {
                member_orgs: vec![ORG],
                admin_orgs: vec![ORG],
                ..base()
            },
        ),
        (
            "org owner",
            PrincipalFacts {
                member_orgs: vec![ORG],
                admin_orgs: vec![ORG],
                owned_orgs: vec![ORG],
                ..base()
            },
        ),
        (
            "global admin, not a member",
            PrincipalFacts {
                platform: Some(operator.clone()),
                ..base()
            },
        ),
        (
            "app operator, not a member",
            PrincipalFacts {
                platform: Some(app_operator),
                ..base()
            },
        ),
        (
            "global admin who is a plain member",
            PrincipalFacts {
                member_orgs: vec![ORG],
                platform: Some(operator),
                ..base()
            },
        ),
        (
            "managing partner",
            PrincipalFacts {
                partners: vec![crate::PartnerStanding {
                    partner_id: Uuid::from_u128(0x900),
                    client_orgs: vec![ORG],
                    caps: vec![Cap::ManageOrgSettings, Cap::DevelopApps],
                }],
                ..base()
            },
        ),
    ]
}

#[test]
fn for_every_person_the_compile_check_is_the_factory_and_the_editor_guard() {
    for (name, person) in people() {
        let mut shapes = vec![("session".to_string(), person.clone())];
        for ceiling in RoleCeiling::ALL {
            for workspace_id in [None, Some(WS)] {
                // A person's token is a personal one: never a CI run. Its
                // standing flags ride along so staff and partner terms are
                // exercised under a ceiling too.
                let reach = TokenReach {
                    platform: true,
                    partner: true,
                    ..token(vec![grant(workspace_id, ceiling)], false)
                };
                shapes.push((
                    format!("{ceiling:?} {workspace_id:?}"),
                    person.clone().narrowed_by(&reach),
                ));
            }
        }
        for (shape, facts) in shapes {
            for resource in resources() {
                let want = allows(&facts, Action::FactoryUse, &resource)
                    && allows(&facts, Action::WorkspaceEdit, &resource);
                assert_eq!(
                    allows(&facts, Action::CompileCheck, &resource),
                    want,
                    "{name} ({shape}) on {resource:?}"
                );
            }
        }
    }
}

#[test]
fn a_ci_run_never_rides_a_sandbox_agent_or_an_apps_credential() {
    // Both narrower credentials are asked first and cover no compile check, so
    // a CI-shaped fact beside them opens nothing.
    let sandbox = TokenReach {
        sandbox_agent: Some(crate::SandboxAgentReach {
            token_id: Uuid::from_u128(0x5),
            apps: Vec::new(),
        }),
        ..ci_viewer()
    };
    let app = TokenReach {
        app: Some(crate::AppReach {
            app_id: APP,
            grants: Vec::new(),
        }),
        ..ci_viewer()
    };
    for reach in [sandbox, app] {
        let facts = PrincipalFacts {
            token: Some(reach),
            ..account_of(ORG, false)
        };
        assert!(!allows(
            &facts,
            Action::CompileCheck,
            &Resource::workspace(WS, ORG)
        ));
    }
}
