//! The **staging option** of a sandbox agent token against the shipped gates,
//! over the app caller shapes of the parent module — beside
//! `differential_sandbox`, whose cases are a token minted without it and
//! stand unchanged.
//!
//! The oracle is the one that module states, with one row more: the shipped
//! check as the token's caller sees it — the minter's `develop_apps` reach for
//! `AppNonProduction`, the minter's staff `manage_apps` for `AppAdmin` — ANDed
//! with the cover, which now also holds on **staging of an app granted
//! staging**. Production, and a decision that names no environment, are
//! covered by nothing.
//!
//! The token's facts are built the way a request builds them: a
//! [`CredentialContext`] whose `app_sandbox` grant carries `staging`, through
//! [`Caller`], through [`Caller::narrow`].

use oxy_auth::token::{AppSandboxGrant, CredentialContext, StoredKind};
use oxy_auth::types::AuthenticatedUser;
use oxy_authz::{
    Action, Cap, EnvFacet, PlatformRole, PlatformStanding, PrincipalFacts, Resource, RoleCeiling,
    Scope, TokenGrant, allows,
};
use uuid::Uuid;

use super::{app_id, app_scenarios, org, user, ws_id};
use crate::caller::Caller;

const TOKEN: Uuid = Uuid::from_u128(0x5B11);
const OTHER_TOKEN: Uuid = Uuid::from_u128(0x5B12);

const OWN: EnvFacet = EnvFacet::Sandbox {
    created_by_token: Some(TOKEN),
};
const FACETS: [EnvFacet; 6] = [
    EnvFacet::Production,
    EnvFacet::Staging,
    OWN,
    EnvFacet::Sandbox {
        created_by_token: Some(OTHER_TOKEN),
    },
    EnvFacet::Sandbox {
        created_by_token: None,
    },
    EnvFacet::NewSandbox,
];
const TENANT_ACTIONS: [Action; 2] = [Action::AppNonProduction, Action::AppAdmin];

/// Granted, of the same workspace, **without** staging.
fn plain_app() -> Uuid {
    Uuid::from_u128(953)
}

/// Not granted at all, of the same workspace.
fn sibling_app() -> Uuid {
    Uuid::from_u128(952)
}

fn another_org() -> Uuid {
    Uuid::from_u128(2)
}

/// The credential admission hands a request that presented a sandbox agent
/// token granted `app_id()` with staging and `plain_app()` without.
fn credential(staging: bool) -> CredentialContext {
    let grant = |app_id, staging| AppSandboxGrant {
        org_id: org(),
        app_id,
        staging,
    };
    CredentialContext {
        token_id: TOKEN,
        kind: StoredKind::SandboxAgent,
        principal_user_id: user(),
        all_access: false,
        platform: true,
        partner: false,
        name: "agent".into(),
        display_prefix: "oxy_sbx_Ab3x".into(),
        legacy_api_key_id: None,
        grants: vec![TokenGrant {
            org_id: org(),
            workspace_id: Some(ws_id()),
            ceiling: RoleCeiling::Admin,
        }],
        app_publish: Vec::new(),
        app_sandbox: vec![grant(app_id(), staging), grant(plain_app(), false)],
        blocked_orgs: Vec::new(),
        service_account: None,
        expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(8)),
    }
}

fn token_caller(staging: bool) -> Caller {
    let user = AuthenticatedUser {
        id: user(),
        email: Some("minter@oxy.tech".into()),
        name: "Minter".into(),
        picture: None,
        status: entity::users::UserStatus::Active,
        credential: None,
    };
    Caller::of(&user, Some(&credential(staging)))
}

/// `minter`'s facts as a request on the staging-granted token carries them.
fn through_token(minter: PrincipalFacts) -> PrincipalFacts {
    token_caller(true).narrow(minter)
}

fn app_in(app: Uuid, facet: EnvFacet) -> Resource {
    Resource::app_environment(app, org(), facet).published_from(ws_id())
}

/// Whether the staging-granted token covers `facet` of `app` for `action` —
/// the table of the design's "Staging option", restated here so a change to
/// the model has to change it too.
fn covered(action: Action, app: Uuid, facet: EnvFacet) -> bool {
    let granted = app == app_id() || app == plain_app();
    let staged = app == app_id() && facet == EnvFacet::Staging;
    match action {
        Action::AppNonProduction => {
            staged || (granted && (facet == OWN || facet == EnvFacet::NewSandbox))
        }
        Action::AppAdmin => staged || (granted && facet == OWN),
        _ => false,
    }
}

#[test]
fn the_caller_carries_staging_for_the_app_it_was_granted_it_for() {
    let caller = token_caller(true);
    let reach = caller.sandbox_agent().expect("a sandbox agent");
    let staged: Vec<(Uuid, bool)> = reach.apps.iter().map(|a| (a.app_id, a.staging)).collect();
    assert_eq!(staged, vec![(app_id(), true), (plain_app(), false)]);
    let plain = token_caller(false);
    let reach = plain.sandbox_agent().expect("a sandbox agent");
    assert!(reach.apps.iter().all(|app| !app.staging));
}

#[test]
fn with_staging_non_production_is_the_minters_reach_and_the_cover() {
    for s in app_scenarios() {
        // Oracle = `platform_reaches(caller, DevelopApps, app.org_id)` as the
        // token's caller reads it. The scenarios' staff is a Global Admin.
        let minters_gate = s.is_staff;
        let facts = through_token(s.facts());
        for app in [app_id(), plain_app(), sibling_app()] {
            for facet in FACETS {
                let expected = minters_gate && covered(Action::AppNonProduction, app, facet);
                assert_eq!(
                    allows(&facts, Action::AppNonProduction, &app_in(app, facet)),
                    expected,
                    "AppNonProduction {facet:?} app={app} — scenario {:?}",
                    s.name
                );
            }
            let unfaceted = Resource::app(app, org()).published_from(ws_id());
            assert!(
                !allows(&facts, Action::AppNonProduction, &unfaceted),
                "an unfaceted decision passed — scenario {:?}",
                s.name
            );
        }
    }
}

#[test]
fn with_staging_app_admin_is_the_minters_staff_standing_and_the_cover() {
    for s in app_scenarios() {
        // Oracle = `resolve_app_role`'s admin verdict as the token's caller
        // reads it: the token carries no tenant standing, so what is left is
        // the staff term.
        let minters_gate = s.is_staff;
        let facts = through_token(s.facts());
        for app in [app_id(), plain_app(), sibling_app()] {
            for facet in FACETS {
                let expected = minters_gate && covered(Action::AppAdmin, app, facet);
                assert_eq!(
                    allows(&facts, Action::AppAdmin, &app_in(app, facet)),
                    expected,
                    "AppAdmin {facet:?} app={app} — scenario {:?}",
                    s.name
                );
            }
        }
    }
}

/// Granted staging, the token decides on staging exactly as its minter's
/// session does — for every caller shape, not only staff.
#[test]
fn on_granted_staging_the_token_decides_as_a_staff_minter_does() {
    for s in app_scenarios() {
        let (session, token) = (s.facts(), through_token(s.facts()));
        let staging = app_in(app_id(), EnvFacet::Staging);
        // `AppNonProduction` has one term, the staff one: equal for everyone.
        assert_eq!(
            allows(&token, Action::AppNonProduction, &staging),
            allows(&session, Action::AppNonProduction, &staging),
            "scenario {:?}",
            s.name
        );
        // `AppAdmin` also has tenant terms, which the token drops: it is the
        // session's verdict where the minter is staff, and never more.
        if s.is_staff {
            assert!(allows(&token, Action::AppAdmin, &staging), "{:?}", s.name);
        }
        if allows(&token, Action::AppAdmin, &staging) {
            assert!(allows(&session, Action::AppAdmin, &staging), "{:?}", s.name);
        }
    }
}

#[test]
fn production_and_no_facet_are_false_with_staging_granted() {
    for s in app_scenarios() {
        let facts = through_token(s.facts());
        for action in Action::ALL {
            for app in [app_id(), plain_app(), sibling_app()] {
                assert!(
                    !allows(&facts, action, &app_in(app, EnvFacet::Production)),
                    "{action:?} on production of {app} — scenario {:?}",
                    s.name
                );
            }
        }
        for action in TENANT_ACTIONS {
            let unfaceted = Resource::app(app_id(), org()).published_from(ws_id());
            assert!(!allows(&facts, action, &unfaceted), "{:?}", s.name);
        }
    }
}

#[test]
fn granted_without_staging_refuses_staging() {
    for s in app_scenarios() {
        let facts = token_caller(false).narrow(s.facts());
        for action in TENANT_ACTIONS {
            for app in [app_id(), plain_app(), sibling_app()] {
                assert!(
                    !allows(&facts, action, &app_in(app, EnvFacet::Staging)),
                    "{action:?} on staging of {app} — scenario {:?}",
                    s.name
                );
            }
        }
    }
}

#[test]
fn the_wrong_org_refuses_staging() {
    let staff = |scope: Scope| PrincipalFacts {
        user_id: user(),
        platform: Some(PlatformStanding::from_role(
            PlatformRole::AppOperator,
            scope,
        )),
        ..Default::default()
    };
    // The app's id under another org is not the app the token was granted.
    let facts = through_token(staff(Scope::All));
    let elsewhere = Resource::app_environment(app_id(), another_org(), EnvFacet::Staging);
    for action in TENANT_ACTIONS {
        assert!(allows(&facts, action, &app_in(app_id(), EnvFacet::Staging)));
        assert!(!allows(&facts, action, &elsewhere), "{action:?}");
    }
    // A minter whose grant reaches another org only: nothing here, staging
    // included, through the token or not.
    let abroad = staff(Scope::Orgs(vec![another_org()]));
    let facts = through_token(abroad.clone());
    for action in TENANT_ACTIONS {
        let staging = app_in(app_id(), EnvFacet::Staging);
        assert!(!allows(&abroad, action, &staging), "{action:?}");
        assert!(!allows(&facts, action, &staging), "{action:?}");
    }
}

#[test]
fn a_minter_without_develop_apps_has_a_token_refused_on_staging() {
    let staff = |caps: Vec<Cap>| PrincipalFacts {
        user_id: user(),
        platform: Some(PlatformStanding {
            role: PlatformRole::AppOperator,
            caps,
            scope: Scope::All,
        }),
        ..Default::default()
    };
    let staging = app_in(app_id(), EnvFacet::Staging);

    let both = through_token(staff(vec![Cap::DevelopApps, Cap::ManageApps]));
    assert!(allows(&both, Action::AppNonProduction, &staging));
    assert!(allows(&both, Action::AppAdmin, &staging));

    let no_develop = through_token(staff(vec![Cap::ManageApps]));
    assert!(!allows(&no_develop, Action::AppNonProduction, &staging));

    let no_manage = through_token(staff(vec![Cap::DevelopApps]));
    assert!(allows(&no_manage, Action::AppNonProduction, &staging));
    assert!(!allows(&no_manage, Action::AppAdmin, &staging));
}

#[test]
fn the_staging_token_only_ever_subtracts_from_its_minter() {
    let mut resources = vec![
        Resource::platform(),
        Resource::org(org()),
        Resource::workspace(ws_id(), org()),
        Resource::app(app_id(), org()),
        Resource::app(sibling_app(), org()).published_from(ws_id()),
    ];
    for facet in FACETS {
        for app in [app_id(), plain_app(), sibling_app()] {
            resources.push(app_in(app, facet));
        }
        resources.push(Resource::app_environment(app_id(), another_org(), facet));
    }
    for s in app_scenarios() {
        let session = s.facts();
        let token = through_token(s.facts());
        for action in Action::ALL {
            for resource in &resources {
                if allows(&token, action, resource) {
                    assert!(
                        allows(&session, action, resource),
                        "{action:?} on {resource:?} is wider than the session — scenario {:?}",
                        s.name
                    );
                }
            }
        }
    }
}
