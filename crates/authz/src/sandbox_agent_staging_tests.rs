//! The staging option of a sandbox agent token (`SandboxApp::staging`),
//! proved over `Action::ALL` beside `sandbox_agent_tests`, whose cases — a
//! token minted **without** it — stand unchanged.
//!
//! Four statements:
//!
//! 1. granted staging, the two tenant actions also cover `Staging` of that
//!    app, and decide there as the minter does;
//! 2. production, and a decision that names no environment, are covered by
//!    nothing, with staging or without;
//! 3. staging is per app and per org: a granted app minted without it, a
//!    sibling app and the same id under another org are refused;
//! 4. the option adds no action, and never decides `true` where the minter
//!    decides `false`.

use uuid::Uuid;

use crate::{
    Action, Cap, EnvFacet, PlatformRole, PlatformStanding, PrincipalFacts, Resource, ResourceKind,
    RoleCeiling, SandboxAgentReach, SandboxApp, Scope, TokenGrant, TokenReach, allows,
};

const ORG: Uuid = Uuid::from_u128(0xA);
const OTHER_ORG: Uuid = Uuid::from_u128(0xB);
const WORKSPACE: Uuid = Uuid::from_u128(0xA1);
/// Granted with staging.
const APP: Uuid = Uuid::from_u128(0xAA);
/// Granted, of the same workspace, **without** staging.
const PLAIN_APP: Uuid = Uuid::from_u128(0xAC);
/// Not granted at all, of the same workspace.
const SIBLING_APP: Uuid = Uuid::from_u128(0xAB);
const MINTER: Uuid = Uuid::from_u128(0x1);
const TOKEN: Uuid = Uuid::from_u128(0x70);

const OWN: EnvFacet = EnvFacet::Sandbox {
    created_by_token: Some(TOKEN),
};
const A_PERSONS: EnvFacet = EnvFacet::Sandbox {
    created_by_token: None,
};
const FACETS: [EnvFacet; 5] = [
    EnvFacet::Production,
    EnvFacet::Staging,
    OWN,
    A_PERSONS,
    EnvFacet::NewSandbox,
];
const TENANT_ACTIONS: [Action; 2] = [Action::AppNonProduction, Action::AppAdmin];

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

fn operator() -> PrincipalFacts {
    staff(PlatformRole::AppOperator.caps())
}

/// Minters of every standing: both capabilities, one of them, and neither.
fn minters() -> [PrincipalFacts; 5] {
    [
        operator(),
        staff(vec![Cap::ManageApps]),
        staff(vec![Cap::DevelopApps]),
        staff(Vec::new()),
        PrincipalFacts {
            user_id: MINTER,
            ..Default::default()
        },
    ]
}

fn granted(app_id: Uuid, staging: bool) -> SandboxApp {
    SandboxApp {
        app_id,
        org_id: ORG,
        staging,
    }
}

/// The reach of a token holding `apps`.
fn reach_of(apps: Vec<SandboxApp>) -> TokenReach {
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
            apps,
        }),
    }
}

/// A token granted `APP` with staging and `PLAIN_APP` without.
fn reach() -> TokenReach {
    reach_of(vec![granted(APP, true), granted(PLAIN_APP, false)])
}

fn through_token(minter: PrincipalFacts) -> PrincipalFacts {
    minter.narrowed_by(&reach())
}

fn app_in(app: Uuid, facet: EnvFacet) -> Resource {
    Resource::app_environment(app, ORG, facet).published_from(WORKSPACE)
}

fn staging(app: Uuid) -> Resource {
    app_in(app, EnvFacet::Staging)
}

fn resources() -> Vec<Resource> {
    let mut out = vec![
        Resource::platform(),
        Resource::org(ORG),
        Resource::workspace(WORKSPACE, ORG),
    ];
    for app in [APP, PLAIN_APP, SIBLING_APP] {
        out.push(Resource::app(app, ORG));
        out.push(Resource::app(app, ORG).published_from(WORKSPACE));
        out.push(Resource::app(app, OTHER_ORG));
        for facet in FACETS {
            out.push(app_in(app, facet));
            out.push(Resource::app_environment(app, ORG, facet));
            out.push(Resource::app_environment(app, OTHER_ORG, facet));
        }
    }
    out
}

#[test]
fn granted_staging_decides_on_staging_as_the_minter_does() {
    for minter in minters() {
        let token = through_token(minter.clone());
        for action in TENANT_ACTIONS {
            assert_eq!(
                allows(&token, action, &staging(APP)),
                allows(&minter, action, &staging(APP)),
                "{action:?} for a minter holding {:?}",
                minter.platform.as_ref().map(|p| &p.caps)
            );
        }
    }
    // For the minter who may mint, that is a yes on both.
    let token = through_token(operator());
    for action in TENANT_ACTIONS {
        assert!(allows(&token, action, &staging(APP)), "{action:?}");
    }
}

#[test]
fn production_and_no_facet_are_refused_with_staging_granted() {
    let token = through_token(operator());
    for action in Action::ALL {
        for app in [APP, PLAIN_APP, SIBLING_APP] {
            assert!(
                !allows(&token, action, &app_in(app, EnvFacet::Production)),
                "{action:?} on production of {app}"
            );
        }
    }
    for action in TENANT_ACTIONS {
        assert!(!allows(&token, action, &Resource::app(APP, ORG)));
        let unfaceted = Resource::app(APP, ORG).published_from(WORKSPACE);
        assert!(!allows(&token, action, &unfaceted), "{action:?}");
    }
}

#[test]
fn staging_is_refused_without_the_grant_for_that_app() {
    let token = through_token(operator());
    for action in TENANT_ACTIONS {
        // A granted app minted without staging: its sandboxes, never its
        // staging.
        assert!(!allows(&token, action, &staging(PLAIN_APP)), "{action:?}");
        assert!(
            allows(&token, action, &app_in(PLAIN_APP, OWN)),
            "{action:?}"
        );
        // An app of the same workspace the token was never granted.
        assert!(!allows(&token, action, &staging(SIBLING_APP)), "{action:?}");
    }
}

#[test]
fn staging_of_the_app_under_another_org_is_refused() {
    let token = through_token(operator());
    let elsewhere = Resource::app_environment(APP, OTHER_ORG, EnvFacet::Staging);
    for action in TENANT_ACTIONS {
        assert!(!allows(&token, action, &elsewhere), "{action:?}");
    }
    // A minter whose standing reaches another org only has a token that opens
    // nothing here, staging included.
    let abroad = PrincipalFacts {
        user_id: MINTER,
        platform: Some(PlatformStanding::from_role(
            PlatformRole::AppOperator,
            Scope::Orgs(vec![OTHER_ORG]),
        )),
        ..Default::default()
    };
    let token = through_token(abroad);
    for action in TENANT_ACTIONS {
        assert!(!allows(&token, action, &staging(APP)), "{action:?}");
    }
}

#[test]
fn a_minter_without_develop_apps_has_a_token_that_opens_no_staging() {
    let token = through_token(staff(vec![Cap::ManageApps]));
    assert!(!allows(&token, Action::AppNonProduction, &staging(APP)));
    // And one without `manage_apps` opens staging, with no admin surface there.
    let token = through_token(staff(vec![Cap::DevelopApps]));
    assert!(allows(&token, Action::AppNonProduction, &staging(APP)));
    assert!(!allows(&token, Action::AppAdmin, &staging(APP)));
}

#[test]
fn staging_adds_no_action_and_no_other_environment() {
    let with = through_token(operator());
    let plain = reach_of(vec![granted(APP, false), granted(PLAIN_APP, false)]);
    let without = operator().narrowed_by(&plain);
    // The one thing the grant adds: staging of `APP`, in its own org.
    let staging_of_app = |r: &Resource| {
        (r.kind, r.id, r.org_id) == (ResourceKind::App, APP, ORG)
            && r.environment == Some(EnvFacet::Staging)
    };
    for action in Action::ALL {
        for resource in resources() {
            if staging_of_app(&resource) && TENANT_ACTIONS.contains(&action) {
                assert!(allows(&with, action, &resource), "{action:?} {resource:?}");
                assert!(!allows(&without, action, &resource), "{action:?}");
                continue;
            }
            assert_eq!(
                allows(&with, action, &resource),
                allows(&without, action, &resource),
                "{action:?} on {resource:?} moved with the staging grant"
            );
        }
    }
}

#[test]
fn the_token_never_decides_true_where_its_minter_decides_false() {
    for minter in minters() {
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
fn stages_app_is_by_app_org_grant_and_facet() {
    let sandbox = reach().sandbox_agent.expect("a sandbox agent reach");
    assert!(sandbox.stages_app(&staging(APP)));
    assert!(!sandbox.stages_app(&staging(PLAIN_APP)));
    assert!(!sandbox.stages_app(&staging(SIBLING_APP)));
    let elsewhere = Resource::app_environment(APP, OTHER_ORG, EnvFacet::Staging);
    assert!(!sandbox.stages_app(&elsewhere));
    for not_staging in [EnvFacet::Production, OWN, A_PERSONS, EnvFacet::NewSandbox] {
        assert!(
            !sandbox.stages_app(&app_in(APP, not_staging)),
            "{not_staging:?}"
        );
    }
    // No facet is not staging, and a workspace whose id equals the app's is
    // not the app.
    assert!(!sandbox.stages_app(&Resource::app(APP, ORG)));
    let workspace = Resource::workspace(APP, ORG).in_environment(EnvFacet::Staging);
    assert!(!sandbox.stages_app(&workspace));
}
