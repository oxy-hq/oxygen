//! The sandbox agent token's reach, proved over `Action::ALL` (sandbox agent
//! credential design §3.2, §7.2).
//!
//! Three statements:
//!
//! 1. the token covers four actions and nothing else, and the two tenant ones
//!    only where the call site names an environment that is the token's own;
//! 2. it never decides `true` where its minter's session decides `false` — it
//!    only subtracts;
//! 3. an environment facet changes nothing for a caller without this token.

use uuid::Uuid;

use crate::{
    Action, Cap, EnvFacet, PlatformRole, PlatformStanding, PrincipalFacts, Resource, RoleCeiling,
    SandboxAgentReach, SandboxApp, Scope, TokenGrant, TokenReach, allows,
};

const ORG: Uuid = Uuid::from_u128(0xA);
const OTHER_ORG: Uuid = Uuid::from_u128(0xB);
const WORKSPACE: Uuid = Uuid::from_u128(0xA1);
const APP: Uuid = Uuid::from_u128(0xAA);
/// Another app of `ORG`, published from the same workspace as `APP`.
const SIBLING_APP: Uuid = Uuid::from_u128(0xAB);
const MINTER: Uuid = Uuid::from_u128(0x1);
const TOKEN: Uuid = Uuid::from_u128(0x70);
const OTHER_TOKEN: Uuid = Uuid::from_u128(0x71);

const OWN: EnvFacet = EnvFacet::Sandbox {
    created_by_token: Some(TOKEN),
};
const ANOTHER_TOKENS: EnvFacet = EnvFacet::Sandbox {
    created_by_token: Some(OTHER_TOKEN),
};
const A_PERSONS: EnvFacet = EnvFacet::Sandbox {
    created_by_token: None,
};
const FACETS: [EnvFacet; 6] = [
    EnvFacet::Production,
    EnvFacet::Staging,
    OWN,
    ANOTHER_TOKENS,
    A_PERSONS,
    EnvFacet::NewSandbox,
];

/// A staff member holding `caps` over `ORG`, in a browser session.
fn staff(caps: Vec<Cap>) -> PrincipalFacts {
    PrincipalFacts {
        user_id: MINTER,
        platform: Some(PlatformStanding {
            role: PlatformRole::AppOperator,
            caps,
            scope: Scope::Orgs(vec![ORG]),
        }),
        ..Default::default()
    }
}

/// An App Operator over `ORG`: `develop_apps` and `manage_apps`, which is who
/// may mint.
fn operator() -> PrincipalFacts {
    staff(PlatformRole::AppOperator.caps())
}

/// The reach admission gives a token granted `APP`: one workspace grant on the
/// app's workspace at admin, and the sandbox fact.
fn reach() -> TokenReach {
    TokenReach {
        all_access: false,
        platform: true,
        partner: false,
        grants: vec![TokenGrant {
            org_id: ORG,
            workspace_id: Some(WORKSPACE),
            ceiling: RoleCeiling::Admin,
        }],
        blocked_orgs: Vec::new(),
        sandbox_agent: Some(SandboxAgentReach {
            token_id: TOKEN,
            apps: vec![SandboxApp {
                app_id: APP,
                org_id: ORG,
                staging: false,
            }],
        }),
    }
}

/// `minter`'s facts as the token carries them.
fn through_token(minter: PrincipalFacts) -> PrincipalFacts {
    minter.narrowed_by(&reach())
}

fn app_in(facet: EnvFacet) -> Resource {
    Resource::app_environment(APP, ORG, facet)
}

/// Every resource shape a decision can be asked of, with and without a facet.
fn resources() -> Vec<Resource> {
    let mut out = vec![
        Resource::platform(),
        Resource::org(ORG),
        Resource::workspace(WORKSPACE, ORG),
        Resource::app(APP, ORG),
        Resource::app(APP, ORG).published_from(WORKSPACE),
        Resource::app_with_visibility(APP, ORG, true),
        Resource::app(SIBLING_APP, ORG).published_from(WORKSPACE),
        Resource::app(APP, OTHER_ORG),
    ];
    for facet in FACETS {
        out.push(app_in(facet));
        out.push(app_in(facet).published_from(WORKSPACE));
        out.push(Resource::app_environment(SIBLING_APP, ORG, facet));
        out.push(Resource::app_environment(APP, OTHER_ORG, facet));
    }
    out
}

#[test]
fn the_token_covers_four_actions_and_nothing_else() {
    let token = through_token(operator());
    let covered = [
        Action::PlatformOps,
        Action::PlatformApps,
        Action::AppNonProduction,
        Action::AppAdmin,
    ];
    for action in Action::ALL {
        if covered.contains(&action) {
            continue;
        }
        for resource in resources() {
            assert!(
                !allows(&token, action, &resource),
                "{action:?} must be refused on {resource:?}"
            );
        }
    }
}

#[test]
fn the_console_doors_open_on_the_platform_singleton_only() {
    let token = through_token(operator());
    for action in [Action::PlatformOps, Action::PlatformApps] {
        assert!(allows(&token, action, &Resource::platform()), "{action:?}");
        for resource in resources().into_iter().skip(1) {
            assert!(
                !allows(&token, action, &resource),
                "{action:?} {resource:?}"
            );
        }
    }
}

#[test]
fn non_production_is_an_own_sandbox_or_a_new_one_of_a_granted_app() {
    let token = through_token(operator());
    let open = |r: &Resource| allows(&token, Action::AppNonProduction, r);
    assert!(open(&app_in(OWN)));
    assert!(open(&app_in(EnvFacet::NewSandbox)));
    assert!(open(&app_in(OWN).published_from(WORKSPACE)));
    for refused in [
        EnvFacet::Production,
        EnvFacet::Staging,
        ANOTHER_TOKENS,
        A_PERSONS,
    ] {
        assert!(!open(&app_in(refused)), "{refused:?}");
    }
    // A call site that names no environment is refused, not waved through.
    assert!(!open(&Resource::app(APP, ORG)));
    assert!(!open(&Resource::app(APP, ORG).published_from(WORKSPACE)));
}

#[test]
fn app_admin_is_an_own_sandbox_only() {
    let token = through_token(operator());
    let admin = |r: &Resource| allows(&token, Action::AppAdmin, r);
    assert!(admin(&app_in(OWN)));
    assert!(admin(&app_in(OWN).published_from(WORKSPACE)));
    for refused in [
        EnvFacet::Production,
        EnvFacet::Staging,
        ANOTHER_TOKENS,
        A_PERSONS,
        // Creating or listing is not the admin surface of any one sandbox.
        EnvFacet::NewSandbox,
    ] {
        assert!(!admin(&app_in(refused)), "{refused:?}");
    }
    assert!(!admin(&Resource::app(APP, ORG)));
}

#[test]
fn another_app_is_refused_even_from_the_same_workspace() {
    let token = through_token(operator());
    // The workspace grant covers the sibling's workspace; the sandbox fact is
    // what refuses it.
    for facet in [OWN, EnvFacet::NewSandbox] {
        for action in [Action::AppNonProduction, Action::AppAdmin] {
            let sibling =
                Resource::app_environment(SIBLING_APP, ORG, facet).published_from(WORKSPACE);
            assert!(!allows(&token, action, &sibling), "{action:?} {facet:?}");
            let elsewhere = Resource::app_environment(APP, OTHER_ORG, facet);
            assert!(!allows(&token, action, &elsewhere), "{action:?} {facet:?}");
        }
    }
}

#[test]
fn the_token_never_decides_true_where_its_minter_decides_false() {
    // Minters of every standing: both capabilities, one of them, and neither.
    let minters = [
        operator(),
        staff(vec![Cap::ManageApps]),
        staff(vec![Cap::DevelopApps]),
        staff(Vec::new()),
        PrincipalFacts {
            user_id: MINTER,
            ..Default::default()
        },
    ];
    for minter in minters {
        let token = through_token(minter.clone());
        for action in Action::ALL {
            for resource in resources() {
                if allows(&token, action, &resource) {
                    assert!(
                        allows(&minter, action, &resource),
                        "{action:?} on {resource:?} is wider than its minter"
                    );
                }
            }
        }
    }
}

#[test]
fn a_minter_without_develop_apps_has_a_token_that_opens_no_sandbox() {
    let token = through_token(staff(vec![Cap::ManageApps]));
    assert!(!allows(&token, Action::AppNonProduction, &app_in(OWN)));
    assert!(!allows(
        &token,
        Action::AppNonProduction,
        &app_in(EnvFacet::NewSandbox)
    ));
    // And one without `manage_apps` holds no admin surface on its own sandbox.
    let token = through_token(staff(vec![Cap::DevelopApps]));
    assert!(!allows(&token, Action::AppAdmin, &app_in(OWN)));
}

#[test]
fn a_facet_changes_nothing_without_this_token() {
    // The "a session decides as before" proof: for a session, and for a
    // personal token that is not a sandbox agent's, naming an environment
    // decides exactly as naming none.
    let personal = TokenReach {
        sandbox_agent: None,
        ..reach()
    };
    let member = PrincipalFacts {
        user_id: MINTER,
        member_orgs: vec![ORG],
        admin_orgs: vec![ORG],
        ..Default::default()
    };
    let callers = [
        operator(),
        member.clone(),
        operator().narrowed_by(&personal),
        member.narrowed_by(&personal),
    ];
    for caller in callers {
        for action in Action::ALL {
            for facet in FACETS {
                for app in [APP, SIBLING_APP] {
                    let plain = Resource::app(app, ORG).published_from(WORKSPACE);
                    let faceted = plain.clone().in_environment(facet);
                    assert_eq!(
                        allows(&caller, action, &faceted),
                        allows(&caller, action, &plain),
                        "{action:?} {facet:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn the_token_leaves_its_minter_no_tenant_standing() {
    // A minter who is also an owner of the org, an elevated workspace admin,
    // an app admin and a partner keeps none of it through the token.
    let minter = PrincipalFacts {
        owned_orgs: vec![ORG],
        admin_orgs: vec![ORG],
        member_orgs: vec![ORG],
        ws_admin_override: vec![WORKSPACE],
        app_memberships: vec![APP],
        app_admin_memberships: vec![APP],
        frontline_orgs: vec![ORG],
        frontline_workspace_grants: vec![WORKSPACE],
        ..operator()
    };
    let token = through_token(minter);
    assert!(token.member_orgs.is_empty());
    assert!(token.admin_orgs.is_empty());
    assert!(token.owned_orgs.is_empty());
    assert!(token.ws_admin_override.is_empty());
    assert!(token.app_memberships.is_empty());
    assert!(token.app_admin_memberships.is_empty());
    assert!(token.frontline_orgs.is_empty());
    assert!(token.frontline_workspace_grants.is_empty());
    assert!(token.partners.is_empty());
    assert!(token.service_account.is_none());
    // Staff standing rides along, bounded to the granted apps' orgs.
    assert_eq!(
        token.platform.map(|p| p.scope),
        Some(Scope::Orgs(vec![ORG]))
    );
}

#[test]
fn a_root_minter_is_not_root_through_the_token() {
    let root = PrincipalFacts {
        user_id: MINTER,
        is_global_owner: true,
        ..Default::default()
    };
    let token = through_token(root);
    assert!(!token.is_global_owner);
    // It still runs the loop on its own sandbox, and nothing owner-only.
    assert!(allows(&token, Action::AppNonProduction, &app_in(OWN)));
    assert!(!allows(
        &token,
        Action::PlatformOwnerOnly,
        &Resource::platform()
    ));
}

#[test]
fn ownership_is_by_token_id_and_grant_is_by_app_and_org() {
    let sandbox = reach().sandbox_agent.expect("a sandbox agent reach");
    assert!(sandbox.grants_app(&Resource::app(APP, ORG)));
    assert!(!sandbox.grants_app(&Resource::app(SIBLING_APP, ORG)));
    assert!(!sandbox.grants_app(&Resource::app(APP, OTHER_ORG)));
    // A workspace whose id happens to equal the app's is not the app.
    assert!(!sandbox.grants_app(&Resource::workspace(APP, ORG)));
    assert!(sandbox.owns_sandbox(&app_in(OWN)));
    assert!(!sandbox.owns_sandbox(&app_in(ANOTHER_TOKENS)));
    assert!(!sandbox.owns_sandbox(&app_in(EnvFacet::NewSandbox)));
    assert!(!sandbox.owns_sandbox(&Resource::app_environment(SIBLING_APP, ORG, OWN)));
}
