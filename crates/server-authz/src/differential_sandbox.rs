//! The sandbox agent token against the shipped gates (sandbox agent credential
//! design §7.2), over the app caller shapes of the parent module.
//!
//! Two proofs, and the second is worthless without the first:
//!
//! 1. **Without the token, nothing moved.** Naming an environment on the
//!    resource decides exactly as naming none, for every caller shape and both
//!    actions a facet can matter to. That is "a session decides as before".
//! 2. **With the token, the decision is the minter's gate AND the cover.** The
//!    oracle is the shipped check as the token's caller sees it — the minter's
//!    `develop_apps` reach for `AppNonProduction`, the minter's staff
//!    `manage_apps` for `AppAdmin` — ANDed with "an own sandbox of a granted
//!    app". A minter without the capability has a token that is refused even
//!    on its own sandbox.
//!
//! The token's facts are built the way a request builds them: a
//! [`CredentialContext`] of the kind, through [`Caller`], through
//! [`Caller::narrow`].

use oxy_auth::token::{AppSandboxGrant, CredentialContext, StoredKind};
use oxy_auth::types::AuthenticatedUser;
use oxy_authz::{
    Action, Cap, EnvFacet, PlatformRole, PlatformStanding, PrincipalFacts, Resource, RoleCeiling,
    Scope, TokenGrant, allows,
};
use uuid::Uuid;

use super::{app_id, app_scenarios, org, user, ws_id};
use crate::caller::Caller;

const TOKEN: Uuid = Uuid::from_u128(0x5B01);
const OTHER_TOKEN: Uuid = Uuid::from_u128(0x5B02);

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

fn sibling_app() -> Uuid {
    Uuid::from_u128(952)
}

/// The credential admission hands a request that presented a sandbox agent
/// token granted `app_id()`: its app, and the workspace grant the app stands
/// for.
fn sandbox_credential() -> CredentialContext {
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
        app_sandbox: vec![AppSandboxGrant {
            org_id: org(),
            app_id: app_id(),
        }],
        blocked_orgs: Vec::new(),
        service_account: None,
        expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(8)),
    }
}

fn token_caller() -> Caller {
    let user = AuthenticatedUser {
        id: user(),
        email: Some("minter@oxy.tech".into()),
        name: "Minter".into(),
        picture: None,
        status: entity::users::UserStatus::Active,
        credential: None,
    };
    Caller::of(&user, Some(&sandbox_credential()))
}

/// `minter`'s facts as a request on the token carries them.
fn through_token(minter: PrincipalFacts) -> PrincipalFacts {
    token_caller().narrow(minter)
}

fn app_in(app: Uuid, facet: EnvFacet) -> Resource {
    Resource::app_environment(app, org(), facet).published_from(ws_id())
}

/// Whether the token covers `facet` of `app` for `action` — the table of
/// design §3.2, restated here so a change to the model has to change it too.
fn covered(action: Action, app: Uuid, facet: EnvFacet) -> bool {
    let granted = app == app_id();
    match action {
        Action::AppNonProduction => granted && (facet == OWN || facet == EnvFacet::NewSandbox),
        Action::AppAdmin => granted && facet == OWN,
        _ => false,
    }
}

#[test]
fn the_caller_knows_it_is_a_sandbox_agent_and_nothing_else_is() {
    let caller = token_caller();
    assert!(caller.is_sandbox_agent());
    assert_eq!(caller.sandbox_agent().map(|s| s.token_id), Some(TOKEN));
    assert!(caller.bound_to_grants() && caller.carries_platform() && !caller.carries_partner());
    assert!(!caller.is_service_account());
    assert!(!Caller::without_credential(user(), "minter@oxy.tech").is_sandbox_agent());
}

#[test]
fn without_the_token_a_facet_decides_exactly_as_no_facet_does() {
    for s in app_scenarios() {
        for action in [Action::AppNonProduction, Action::AppAdmin] {
            for restricted in [false, true] {
                let plain = Resource::app_with_visibility(app_id(), org(), restricted)
                    .published_from(ws_id());
                let before = allows(&s.facts(), action, &plain);
                for facet in FACETS {
                    let faceted = plain.clone().in_environment(facet);
                    assert_eq!(
                        allows(&s.facts(), action, &faceted),
                        before,
                        "a session moved: {action:?} {facet:?} — scenario {:?}",
                        s.name
                    );
                }
            }
        }
    }
}

#[test]
fn with_the_token_non_production_is_the_minters_reach_and_the_cover() {
    for s in app_scenarios() {
        // Oracle = `platform_reaches(caller, DevelopApps, app.org_id)` as the
        // token's caller reads it: the minter's staff standing over the orgs
        // of the granted apps. The scenarios' staff is a Global Admin.
        let minters_gate = s.is_staff;
        let facts = through_token(s.facts());
        for app in [app_id(), sibling_app()] {
            for facet in FACETS {
                let expected = minters_gate && covered(Action::AppNonProduction, app, facet);
                assert_eq!(
                    allows(&facts, Action::AppNonProduction, &app_in(app, facet)),
                    expected,
                    "AppNonProduction {facet:?} app={app} — scenario {:?}",
                    s.name
                );
            }
            // A call site that names no environment is refused for the token.
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
fn with_the_token_app_admin_is_the_minters_staff_standing_and_an_own_sandbox() {
    for s in app_scenarios() {
        // Oracle = `resolve_app_role`'s admin verdict as the token's caller
        // reads it. The token carries no tenant standing, so the officer and
        // the per-app admin terms are gone; what is left is the staff term.
        let minters_gate = s.is_staff;
        let facts = through_token(s.facts());
        for app in [app_id(), sibling_app()] {
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

#[test]
fn a_minter_missing_a_capability_has_a_token_refused_on_its_own_sandbox() {
    let staff = |caps: Vec<Cap>| PrincipalFacts {
        user_id: user(),
        platform: Some(PlatformStanding {
            role: PlatformRole::AppOperator,
            caps,
            scope: Scope::All,
        }),
        ..Default::default()
    };
    let own = app_in(app_id(), OWN);

    let both = through_token(staff(vec![Cap::DevelopApps, Cap::ManageApps]));
    assert!(allows(&both, Action::AppNonProduction, &own));
    assert!(allows(&both, Action::AppAdmin, &own));

    let no_develop = through_token(staff(vec![Cap::ManageApps]));
    assert!(!allows(&no_develop, Action::AppNonProduction, &own));

    let no_manage = through_token(staff(vec![Cap::DevelopApps]));
    assert!(!allows(&no_manage, Action::AppAdmin, &own));

    // A grant scoped to another org reaches nothing here, through the token or not.
    let elsewhere = through_token(PrincipalFacts {
        user_id: user(),
        platform: Some(PlatformStanding::from_role(
            PlatformRole::AppOperator,
            Scope::Orgs(vec![Uuid::from_u128(2)]),
        )),
        ..Default::default()
    });
    assert!(!allows(&elsewhere, Action::AppNonProduction, &own));
}

#[test]
fn the_token_only_ever_subtracts_from_its_minter() {
    let mut resources = vec![
        Resource::platform(),
        Resource::org(org()),
        Resource::workspace(ws_id(), org()),
        Resource::app(app_id(), org()),
        Resource::app(sibling_app(), org()).published_from(ws_id()),
    ];
    for facet in FACETS {
        resources.push(app_in(app_id(), facet));
        resources.push(app_in(sibling_app(), facet));
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

#[test]
fn the_token_holds_no_tenant_standing_whatever_its_minter_is() {
    for s in app_scenarios() {
        let facts = through_token(s.facts());
        assert!(
            facts.member_orgs.is_empty()
                && facts.admin_orgs.is_empty()
                && facts.owned_orgs.is_empty()
                && facts.app_memberships.is_empty()
                && facts.app_admin_memberships.is_empty()
                && facts.frontline_orgs.is_empty()
                && facts.frontline_workspace_grants.is_empty()
                && facts.partners.is_empty(),
            "tenant standing survived the token — scenario {:?}",
            s.name
        );
        // So every tenant action that is not one of the two covered ones is
        // refused on the granted app's own workspace and org.
        for action in [
            Action::OrgRead,
            Action::WorkspaceEdit,
            Action::WorkspaceManage,
            Action::AppAccess,
            Action::WorkspaceDataAccess,
        ] {
            for resource in [
                Resource::org(org()),
                Resource::workspace(ws_id(), org()),
                app_in(app_id(), OWN),
            ] {
                assert!(
                    !allows(&facts, action, &resource),
                    "{action:?} passed on {resource:?} — scenario {:?}",
                    s.name
                );
            }
        }
    }
}
