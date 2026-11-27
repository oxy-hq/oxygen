//! The platform checker (`PlatformRole::AppChecker`): it opens a custom app's
//! staging read-only (`Action::AppCheck`) and its production read-only
//! (`Action::AppCheckProduction`), each on its own environment and no other,
//! and is refused everything else.

use uuid::Uuid;

use crate::{
    Action, Cap, EnvFacet, PlatformRole, PlatformStanding, PrincipalFacts, Resource, RoleCeiling,
    Scope, TokenGrant, TokenReach, allows,
};

fn org() -> Uuid {
    Uuid::from_u128(0xa1)
}

fn app() -> Uuid {
    Uuid::from_u128(0xb1)
}

fn standing(role: PlatformRole, scope: Scope) -> PrincipalFacts {
    PrincipalFacts {
        user_id: Uuid::from_u128(0xc1),
        platform: Some(PlatformStanding::from_role(role, scope)),
        ..PrincipalFacts::default()
    }
}

fn checker() -> PrincipalFacts {
    standing(PlatformRole::AppChecker, Scope::All)
}

fn staging() -> Resource {
    Resource::app_environment(app(), org(), EnvFacet::Staging)
}

/// Every resource shape a decision can name, the app in each environment.
fn every_resource() -> Vec<(&'static str, Resource)> {
    let ws = Uuid::from_u128(0xd1);
    vec![
        ("org", Resource::org(org())),
        ("workspace", Resource::workspace(ws, org())),
        ("platform", Resource::platform()),
        ("app, no environment", Resource::app(app(), org())),
        ("app staging", staging()),
        (
            "app production",
            Resource::app_environment(app(), org(), EnvFacet::Production),
        ),
        (
            "app sandbox",
            Resource::app_environment(
                app(),
                org(),
                EnvFacet::Sandbox {
                    created_by_token: None,
                },
            ),
        ),
        (
            "app new sandbox",
            Resource::app_environment(app(), org(), EnvFacet::NewSandbox),
        ),
    ]
}

fn production() -> Resource {
    Resource::app_environment(app(), org(), EnvFacet::Production)
}

#[test]
fn the_checker_opens_an_apps_staging_and_its_production_and_nothing_else() {
    let facts = checker();
    for action in Action::ALL {
        for (name, resource) in every_resource() {
            let expected = match action {
                Action::AppCheck => name == "app staging",
                Action::AppCheckProduction => name == "app production",
                _ => false,
            };
            assert_eq!(
                allows(&facts, action, &resource),
                expected,
                "the checker on {action:?} over {name}"
            );
        }
    }
}

/// Production's check is the staging one's twin: the same capability, held
/// to the app's org and to an admin ceiling, and never the other environment.
#[test]
fn the_production_check_is_held_to_production_its_scope_and_an_admin_ceiling() {
    let other = Uuid::from_u128(0xa2);
    let scoped = standing(PlatformRole::AppChecker, Scope::Orgs(vec![org()]));
    assert!(allows(&scoped, Action::AppCheckProduction, &production()));
    let elsewhere = standing(PlatformRole::AppChecker, Scope::Orgs(vec![other]));
    assert!(!allows(
        &elsewhere,
        Action::AppCheckProduction,
        &production()
    ));
    // Each action names one environment: neither reaches the other's.
    assert!(!allows(&checker(), Action::AppCheckProduction, &staging()));
    assert!(!allows(&checker(), Action::AppCheck, &production()));
    // An App Operator holds no `check_apps`; a Global Admin holds it.
    let operator = standing(PlatformRole::AppOperator, Scope::All);
    assert!(!allows(
        &operator,
        Action::AppCheckProduction,
        &production()
    ));
    let admin = standing(PlatformRole::GlobalAdmin, Scope::All);
    assert!(allows(&admin, Action::AppCheckProduction, &production()));
    assert!(!allows(
        &PrincipalFacts::default(),
        Action::AppCheckProduction,
        &production()
    ));
    // A token carries it only at an admin ceiling, and only over this org.
    assert!(allows(
        &on_a_token(RoleCeiling::Admin),
        Action::AppCheckProduction,
        &production()
    ));
    for below in [RoleCeiling::Viewer, RoleCeiling::Member] {
        assert!(
            !allows(
                &on_a_token(below),
                Action::AppCheckProduction,
                &production()
            ),
            "{below:?}"
        );
    }
    let another_orgs = Resource::app_environment(app(), other, EnvFacet::Production);
    assert!(!allows(
        &on_a_token(RoleCeiling::Admin),
        Action::AppCheckProduction,
        &another_orgs
    ));
    // An org's own people never hold it: it is the platform checking.
    let owner = PrincipalFacts {
        user_id: Uuid::from_u128(0xc2),
        member_orgs: vec![org()],
        owned_orgs: vec![org()],
        ..PrincipalFacts::default()
    };
    assert!(!allows(&owner, Action::AppCheckProduction, &production()));
}

#[test]
fn the_checker_is_held_to_its_scope_and_to_one_capability() {
    let other = Uuid::from_u128(0xa2);
    let scoped = standing(PlatformRole::AppChecker, Scope::Orgs(vec![org()]));
    assert!(allows(&scoped, Action::AppCheck, &staging()));
    let elsewhere = standing(PlatformRole::AppChecker, Scope::Orgs(vec![other]));
    assert!(!allows(&elsewhere, Action::AppCheck, &staging()));
    assert_eq!(PlatformRole::AppChecker.caps(), vec![Cap::CheckApps]);
    // An App Operator ships and develops apps; checking a build is not in it.
    let operator = standing(PlatformRole::AppOperator, Scope::All);
    assert!(!allows(&operator, Action::AppCheck, &staging()));
    // A Global Admin holds every grantable capability, this one included.
    let admin = standing(PlatformRole::GlobalAdmin, Scope::All);
    assert!(allows(&admin, Action::AppCheck, &staging()));
    // No standing, no check.
    assert!(!allows(
        &PrincipalFacts::default(),
        Action::AppCheck,
        &staging()
    ));
}

#[test]
fn the_checker_role_is_a_services_and_never_a_grant_rows() {
    assert!(!PlatformRole::AppChecker.is_grantable());
    assert!(PlatformRole::AppOperator.is_grantable());
    assert!(PlatformRole::GlobalAdmin.is_grantable());
    assert_eq!(
        PlatformRole::from_str("app_checker"),
        Some(PlatformRole::AppChecker)
    );
    assert_eq!(Cap::from_str("check_apps"), Some(Cap::CheckApps));
    assert!(PlatformRole::AppChecker.rank() < PlatformRole::AppOperator.rank());
}

/// The checker's facts as a token of `ceiling` over the app's org carries them.
fn on_a_token(ceiling: RoleCeiling) -> PrincipalFacts {
    checker().narrowed_by(&TokenReach {
        all_access: false,
        platform: true,
        partner: false,
        ci_run: false,
        grants: vec![TokenGrant {
            org_id: org(),
            workspace_id: None,
            ceiling,
        }],
        blocked_orgs: Vec::new(),
        sandbox_agent: None,
        app: None,
    })
}

#[test]
fn a_token_carries_the_check_only_at_an_admin_ceiling() {
    // The check records verdicts as well as reading, so a credential narrowed
    // below admin does not carry it, as it does not carry a staff tool.
    assert!(allows(
        &on_a_token(RoleCeiling::Admin),
        Action::AppCheck,
        &staging()
    ));
    for below in [RoleCeiling::Viewer, RoleCeiling::Member] {
        assert!(
            !allows(&on_a_token(below), Action::AppCheck, &staging()),
            "{below:?}"
        );
    }
    // A token over another org carries nothing here, whatever its ceiling.
    let elsewhere = Resource::app_environment(app(), Uuid::from_u128(0xa2), EnvFacet::Staging);
    assert!(!allows(
        &on_a_token(RoleCeiling::Admin),
        Action::AppCheck,
        &elsewhere
    ));
}

#[test]
fn only_the_checkers_own_standing_is_kept_from_the_console_door() {
    assert!(checker().checks_apps_only());
    assert!(!allows(
        &checker(),
        Action::PlatformOps,
        &Resource::platform()
    ));
    // Staff who also check apps, and a standing that names no capability,
    // are at the door as they always were.
    for role in [PlatformRole::AppOperator, PlatformRole::GlobalAdmin] {
        let staff = standing(role, Scope::All);
        assert!(!staff.checks_apps_only(), "{role:?}");
        assert!(allows(&staff, Action::PlatformOps, &Resource::platform()));
    }
    let bare = PrincipalFacts {
        user_id: Uuid::from_u128(0xc1),
        platform: Some(PlatformStanding {
            role: PlatformRole::AppOperator,
            caps: Vec::new(),
            scope: Scope::All,
        }),
        ..PrincipalFacts::default()
    };
    assert!(!bare.checks_apps_only());
    assert!(allows(&bare, Action::PlatformOps, &Resource::platform()));
    let root = PrincipalFacts {
        user_id: Uuid::from_u128(0xc1),
        is_global_owner: true,
        ..PrincipalFacts::default()
    };
    assert!(!root.checks_apps_only());
}
