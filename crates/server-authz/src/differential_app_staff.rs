//! The app actions staff standing decides, each against the rule it states,
//! over the caller shapes of the parent module: the platform checker on an
//! app's staging (`Action::AppCheck`) and on its production
//! (`Action::AppCheckProduction`), managing an app's events
//! (`Action::AppEventsManage`), and promoting past the promote checks
//! (`Action::AppPromoteOverride`: an officer of the org, or staff standing in
//! as one).

use oxy_authz::{
    Action, Cap, PlatformRole, PlatformStanding, PrincipalFacts, Resource, Scope, allows,
};
use uuid::Uuid;

use entity::org_members::OrgRole;

use super::{app_id, assert_app_ring, org, scenarios, user};

#[test]
fn app_check_ring_matches_the_checker_gate_on_staging_only() {
    // Oracle = `globals::platform_reaches(.., Cap::CheckApps, app.org_id)` on an
    // app's STAGING, the `existing_allow` every door that admits the platform
    // checker passes (`oxy_custom_apps::app_check`: the staging page, the app's
    // own data reads, and the four runner routes that record a build's checks):
    // the global owner, or a grant holding check_apps whose scope reaches the
    // app's org. No tenant standing, and nothing outside staging: production, a
    // sandbox and a decision naming no environment are refused whatever the
    // standing.
    use oxy_authz::EnvFacet;
    for restricted in [false, true] {
        // The app scenarios name no environment, so nobody passes.
        assert_app_ring(Action::AppCheck, restricted, |_| false);
    }
    let other = Uuid::from_u128(2);
    let shapes: Vec<(&str, bool, Option<PlatformStanding>, bool)> = vec![
        ("global owner", true, None, false),
        (
            "app checker",
            false,
            Some(PlatformStanding::from_role(
                PlatformRole::AppChecker,
                Scope::All,
            )),
            false,
        ),
        (
            "app checker scoped to another org",
            false,
            Some(PlatformStanding::from_role(
                PlatformRole::AppChecker,
                Scope::Orgs(vec![other]),
            )),
            false,
        ),
        (
            "app operator",
            false,
            Some(PlatformStanding::from_role(
                PlatformRole::AppOperator,
                Scope::All,
            )),
            false,
        ),
        ("a member of the org", false, None, true),
        ("nobody", false, None, false),
    ];
    let facets = [
        (Some(EnvFacet::Staging), true),
        (Some(EnvFacet::Production), false),
        (
            Some(EnvFacet::Sandbox {
                created_by_token: None,
            }),
            false,
        ),
        (None, false),
    ];
    for (name, is_global_owner, platform, is_member) in shapes {
        for (facet, staging) in facets {
            let reaches = is_global_owner
                || platform
                    .as_ref()
                    .is_some_and(|g| g.grants(Cap::CheckApps, org()));
            let resource = match facet {
                Some(facet) => Resource::app(app_id(), org()).in_environment(facet),
                None => Resource::app(app_id(), org()),
            };
            let facts = PrincipalFacts {
                user_id: user(),
                is_global_owner,
                platform: platform.clone(),
                member_orgs: if is_member { vec![org()] } else { vec![] },
                ..Default::default()
            };
            assert_eq!(
                allows(&facts, Action::AppCheck, &resource),
                reaches && staging,
                "ring drift for AppCheck — scenario {name:?} on {facet:?}"
            );
        }
    }
}

#[test]
fn app_check_production_ring_matches_the_checker_gate_on_production_only() {
    // Oracle = `globals::platform_reaches(.., Cap::CheckApps, app.org_id)` on an
    // app's PRODUCTION, the `existing_allow` every door that admits the platform
    // checker there passes (`oxy_custom_apps::app_check::may_check`: the
    // production page, the app's own data reads, and the runner routes of a
    // production attempt): the global owner, or a grant holding check_apps whose
    // scope reaches the app's org. No tenant standing, and nothing outside
    // production: staging is `AppCheck`'s, and a sandbox and a decision naming
    // no environment are refused whatever the standing.
    use oxy_authz::EnvFacet;
    for restricted in [false, true] {
        // The app scenarios name no environment, so nobody passes.
        assert_app_ring(Action::AppCheckProduction, restricted, |_| false);
    }
    let other = Uuid::from_u128(2);
    let checker = |scope: Scope| Some(PlatformStanding::from_role(PlatformRole::AppChecker, scope));
    let operator = PlatformStanding::from_role(PlatformRole::AppOperator, Scope::All);
    let shapes: Vec<(&str, bool, Option<PlatformStanding>, bool)> = vec![
        ("global owner", true, None, false),
        ("app checker", false, checker(Scope::All), false),
        (
            "app checker scoped to another org",
            false,
            checker(Scope::Orgs(vec![other])),
            false,
        ),
        ("app operator", false, Some(operator), false),
        ("a member of the org", false, None, true),
        ("nobody", false, None, false),
    ];
    let sandbox = EnvFacet::Sandbox {
        created_by_token: None,
    };
    let facets = [
        (Some(EnvFacet::Production), true),
        (Some(EnvFacet::Staging), false),
        (Some(sandbox), false),
        (None, false),
    ];
    for (name, is_global_owner, platform, is_member) in shapes {
        for (facet, production) in facets {
            let reaches = is_global_owner
                || platform
                    .as_ref()
                    .is_some_and(|g| g.grants(Cap::CheckApps, org()));
            let resource = match facet {
                Some(facet) => Resource::app(app_id(), org()).in_environment(facet),
                None => Resource::app(app_id(), org()),
            };
            let facts = PrincipalFacts {
                user_id: user(),
                is_global_owner,
                platform: platform.clone(),
                member_orgs: if is_member { vec![org()] } else { vec![] },
                ..Default::default()
            };
            assert_eq!(
                allows(&facts, Action::AppCheckProduction, &resource),
                reaches && production,
                "ring drift for AppCheckProduction — scenario {name:?} on {facet:?}"
            );
        }
    }
}

#[test]
fn app_events_manage_ring_is_staff_with_manage_apps_over_the_org() {
    // A new surface (the staff endpoints behind `oxyc events`: redeliver,
    // replay, discard, purge, the events switch) with no shipped check, so the
    // oracle is the rule as stated: the global owner, or a grant holding
    // `manage_apps` whose scope reaches the org. No tenant standing: not an
    // officer, an app admin, a member, a partner or a frontline worker.
    for restricted in [false, true] {
        assert_app_ring(Action::AppEventsManage, restricted, |s| s.is_staff);
    }

    let other = Uuid::from_u128(2);
    let scoped = |role, scope| Some(PlatformStanding::from_role(role, scope));
    let shapes: Vec<(&str, bool, Option<PlatformStanding>, bool)> = vec![
        ("global owner", true, None, false),
        (
            "app operator scoped to this org",
            false,
            scoped(PlatformRole::AppOperator, Scope::Orgs(vec![org()])),
            false,
        ),
        (
            "app operator scoped to another org",
            false,
            scoped(PlatformRole::AppOperator, Scope::Orgs(vec![other])),
            false,
        ),
        (
            "app operator scoped to another org who is a member here",
            false,
            scoped(PlatformRole::AppOperator, Scope::Orgs(vec![other])),
            true,
        ),
        (
            "staff without manage_apps",
            false,
            Some(PlatformStanding {
                role: PlatformRole::AppOperator,
                caps: vec![Cap::DevelopApps],
                scope: Scope::All,
            }),
            false,
        ),
        ("a member, not staff", false, None, true),
        ("nobody", false, None, false),
    ];
    for resource in [Resource::org(org()), Resource::app(app_id(), org())] {
        for (name, is_global_owner, platform, is_member) in shapes.clone() {
            let expected = is_global_owner
                || platform
                    .as_ref()
                    .is_some_and(|g| g.grants(Cap::ManageApps, org()));
            let facts = PrincipalFacts {
                user_id: user(),
                is_global_owner,
                platform,
                member_orgs: if is_member { vec![org()] } else { vec![] },
                ..Default::default()
            };
            assert_eq!(
                allows(&facts, Action::AppEventsManage, &resource),
                expected,
                "ring drift for AppEventsManage — scenario {name:?} on {:?}",
                resource.kind
            );
        }
    }
    // A customer's own org owner is refused: tenant standing is not staff.
    let owner = PrincipalFacts {
        user_id: user(),
        owned_orgs: vec![org()],
        admin_orgs: vec![org()],
        member_orgs: vec![org()],
        ..Default::default()
    };
    assert!(!allows(
        &owner,
        Action::AppEventsManage,
        &Resource::org(org())
    ));
}

#[test]
fn app_promote_override_ring_is_an_org_officer_or_staff_standing_in_as_one() {
    // A new decision (`override_checks` on a production move: promote a build
    // past its promote checks) with no shipped check, so the oracle is the rule
    // as stated: a real owner or admin of the app's org, or Oxy staff reaching in
    // through the operator override with `manage_apps`. NOT an `app_members`
    // admin, a plain member, a partner of either capability or a frontline
    // worker. Visibility does not move it.
    for restricted in [false, true] {
        assert_app_ring(Action::AppPromoteOverride, restricted, |s| {
            s.is_staff || s.is_org_owner || s.is_org_admin
        });
    }

    // The org scenarios carry what the app ones hold fixed: what `org_context`
    // derives for the same caller. The override is an Owner or Admin there, real
    // or synthesized for staff; a partner's synthesized Admin is not one, and a
    // staff member who is a plain member keeps their real role.
    let resource = Resource::app(app_id(), org());
    for s in scenarios() {
        let officer = matches!(s.ctx_role, OrgRole::Owner | OrgRole::Admin);
        let expected = officer && (!s.ctx_override || s.facts.is_staff());
        assert_eq!(
            allows(&s.facts, Action::AppPromoteOverride, &resource),
            expected,
            "ring drift for AppPromoteOverride — scenario {:?}",
            s.name
        );
    }

    // Scope and capability, which both scenario sets hold fixed (staff there is
    // an unscoped Global Admin), and the one principal that is not a person.
    let other = Uuid::from_u128(2);
    let scoped = |role, scope| Some(PlatformStanding::from_role(role, scope));
    let shapes: Vec<(&str, bool, Option<PlatformStanding>, bool)> = vec![
        ("global owner", true, None, true),
        (
            "app operator scoped to this org",
            false,
            scoped(PlatformRole::AppOperator, Scope::Orgs(vec![org()])),
            true,
        ),
        (
            "app operator scoped to another org",
            false,
            scoped(PlatformRole::AppOperator, Scope::Orgs(vec![other])),
            false,
        ),
        (
            "staff without manage_apps",
            false,
            Some(PlatformStanding {
                role: PlatformRole::AppOperator,
                caps: vec![Cap::DevelopApps],
                scope: Scope::All,
            }),
            false,
        ),
        (
            "the platform checker",
            false,
            scoped(PlatformRole::AppChecker, Scope::All),
            false,
        ),
        ("nobody", false, None, false),
    ];
    for (name, is_global_owner, platform, expected) in shapes {
        let facts = PrincipalFacts {
            user_id: user(),
            is_global_owner,
            platform,
            ..Default::default()
        };
        assert_eq!(
            allows(&facts, Action::AppPromoteOverride, &resource),
            expected,
            "ring drift for AppPromoteOverride — scenario {name:?}"
        );
    }
    for admin in [false, true] {
        let account = PrincipalFacts {
            user_id: user(),
            service_account: Some(oxy_authz::ServiceAccountStanding {
                org_id: org(),
                admin,
            }),
            ..Default::default()
        };
        assert!(
            !allows(&account, Action::AppPromoteOverride, &resource),
            "the org's service account (admin: {admin}) overrode the checks"
        );
    }
}
