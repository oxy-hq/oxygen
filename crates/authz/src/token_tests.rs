//! The narrowing proofs of API-tokens design §4.4, ring by ring.
//!
//! A session and an all-access token deciding identically proves nothing, so
//! every sweep here runs over `Action::ALL` — which reaches every ring — and
//! asserts the three statements the design makes:
//!
//! 1. a token of ceiling `c` on a principal of role `r` decides like a session
//!    of role `min(r, c)`;
//! 2. a token with no grant on the target decides like a non-member;
//! 3. `platform = false` decides like a principal with no standing (and
//!    `partner = false` like one with no partner standing).

use uuid::Uuid;

use crate::{
    Action, Cap, PartnerStanding, PlatformRole, PlatformStanding, PrincipalFacts, Resource,
    RoleCeiling, Scope, TokenGrant, TokenReach, allows,
};

const ORG: Uuid = Uuid::from_u128(0xA);
const OTHER_ORG: Uuid = Uuid::from_u128(0xB);
const WS_A: Uuid = Uuid::from_u128(0xA1);
const WS_B: Uuid = Uuid::from_u128(0xA2);
const WS_C: Uuid = Uuid::from_u128(0xA3);
const APP: Uuid = Uuid::from_u128(0xAA);
const USER: Uuid = Uuid::from_u128(0x1);
const PARTNER: Uuid = Uuid::from_u128(0xC);

/// A session of `role` in `ORG`. `None` is a non-member.
fn session(role: Option<RoleCeiling>) -> PrincipalFacts {
    let at = |level: RoleCeiling| role.is_some_and(|r| r >= level);
    let set = |on: bool| if on { vec![ORG] } else { Vec::new() };
    PrincipalFacts {
        user_id: USER,
        member_orgs: set(role.is_some()),
        admin_orgs: set(at(RoleCeiling::Admin)),
        owned_orgs: set(at(RoleCeiling::Owner)),
        ..Default::default()
    }
}

/// Every tenant resource shape in `ORG`, including one the user created.
fn tenant_resources() -> Vec<Resource> {
    vec![
        Resource::org(ORG),
        Resource::workspace(WS_A, ORG),
        Resource::workspace_with_creator(WS_A, ORG, Some(USER)),
        Resource::namespace_with_creator(Uuid::from_u128(0xAB), ORG, Some(USER)),
        Resource::app(APP, ORG),
        Resource::app_with_visibility(APP, ORG, true),
        Resource::app(APP, ORG).published_from(WS_A),
    ]
}

/// A plain member of `ORG` holding the per-app admin role on `APP`.
fn app_admin() -> PrincipalFacts {
    PrincipalFacts {
        app_memberships: vec![APP],
        app_admin_memberships: vec![APP],
        ..session(Some(RoleCeiling::Member))
    }
}

fn grant(workspace_id: Option<Uuid>, ceiling: RoleCeiling) -> TokenGrant {
    TokenGrant {
        org_id: ORG,
        workspace_id,
        ceiling,
    }
}

/// A token with these grants that carries both standings.
fn token(grants: Vec<TokenGrant>) -> TokenReach {
    TokenReach {
        all_access: false,
        platform: true,
        partner: true,
        grants,
        blocked_orgs: Vec::new(),
        sandbox_agent: None,
    }
}

/// The two ways a token reaches [`allows`]: capped at the source by the
/// loader, or — had a caller forgotten to — as the bare fact. Both must give
/// the same answer, so the per-decision cap never depends on the loader.
fn both_ways(facts: &PrincipalFacts, token: &TokenReach) -> [PrincipalFacts; 2] {
    [
        facts.clone().narrowed_by(token),
        PrincipalFacts {
            token: Some(token.clone()),
            ..facts.clone()
        },
    ]
}

const ROLES: [RoleCeiling; 3] = [RoleCeiling::Member, RoleCeiling::Admin, RoleCeiling::Owner];

/// The actions a `viewer` loses that a member has, in the model itself: the
/// `WorkspaceEdit` ring ("not a viewer"), and the creator's claim, which is a
/// member's act. Everything else a viewer decides as a member does — the model
/// has no other viewer/member distinction, and the shipped check
/// (`EffectiveWorkspaceRole`, capped at the source) carries the rest.
fn viewer_loses(action: Action, resource: &Resource) -> bool {
    action == Action::WorkspaceEdit
        || (matches!(action, Action::WorkspaceRename | Action::NamespaceDelete)
            && resource.owner == Some(USER))
}

#[test]
fn a_token_of_ceiling_c_on_role_r_decides_like_a_session_of_role_min_r_c() {
    for role in ROLES {
        for ceiling in RoleCeiling::ALL {
            let token = token(vec![grant(None, ceiling)]);
            let capped = role.min(ceiling);
            for facts in both_ways(&session(Some(role)), &token) {
                for action in Action::ALL {
                    for resource in tenant_resources() {
                        let got = allows(&facts, action, &resource);
                        let want = if capped == RoleCeiling::Viewer {
                            allows(&session(Some(RoleCeiling::Member)), action, &resource)
                                && !viewer_loses(action, &resource)
                        } else {
                            allows(&session(Some(capped)), action, &resource)
                        };
                        assert_eq!(
                            got, want,
                            "role {role:?}, ceiling {ceiling:?}, {action:?} on {:?}",
                            resource.kind
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn a_ceiling_takes_something_away_at_every_level() {
    // The sweep above compares two calls to `allows`; this pins that the cap
    // is not vacuous — each ceiling really does deny what the role alone held.
    let owner = session(Some(RoleCeiling::Owner));
    let decide = |ceiling: RoleCeiling, action: Action, resource: Resource| {
        let facts = owner
            .clone()
            .narrowed_by(&token(vec![grant(None, ceiling)]));
        allows(&facts, action, &resource)
    };
    assert!(allows(&owner, Action::OrgOwnerManage, &Resource::org(ORG)));
    assert!(!decide(
        RoleCeiling::Admin,
        Action::OrgOwnerManage,
        Resource::org(ORG)
    ));
    assert!(decide(
        RoleCeiling::Admin,
        Action::MemberSetRole,
        Resource::org(ORG)
    ));
    assert!(!decide(
        RoleCeiling::Member,
        Action::MemberSetRole,
        Resource::org(ORG)
    ));
    assert!(!decide(
        RoleCeiling::Member,
        Action::OrgBilling,
        Resource::org(ORG)
    ));
    let ws = || Resource::workspace(WS_A, ORG);
    assert!(!decide(RoleCeiling::Member, Action::WorkspaceManage, ws()));
    assert!(decide(RoleCeiling::Member, Action::WorkspaceEdit, ws()));
    assert!(!decide(RoleCeiling::Viewer, Action::WorkspaceEdit, ws()));
    assert!(decide(RoleCeiling::Viewer, Action::OrgRead, ws()));
}

#[test]
fn a_ceiling_is_per_workspace_not_per_org() {
    // Admin in A, viewer in B, nothing in C, and no org-wide grant — the case
    // an org set cannot express and the per-decision cap exists for.
    let token = token(vec![
        grant(Some(WS_A), RoleCeiling::Admin),
        grant(Some(WS_B), RoleCeiling::Viewer),
    ]);
    for facts in both_ways(&session(Some(RoleCeiling::Owner)), &token) {
        let on = |ws: Uuid, action: Action| allows(&facts, action, &Resource::workspace(ws, ORG));
        assert!(on(WS_A, Action::WorkspaceManage));
        assert!(on(WS_A, Action::WorkspaceEdit));
        assert!(!on(WS_B, Action::WorkspaceManage));
        assert!(!on(WS_B, Action::WorkspaceEdit));
        assert!(on(WS_B, Action::OrgRead));
        for action in Action::ALL {
            assert!(!on(WS_C, action), "{action:?} on an ungranted workspace");
            // The org's own routes need an org-wide grant.
            assert!(
                !allows(&facts, action, &Resource::org(ORG)),
                "{action:?} on the org with workspace grants only"
            );
        }
    }
}

#[test]
fn a_workspace_elevation_is_capped_like_an_org_role() {
    let elevated = PrincipalFacts {
        ws_admin_override: vec![WS_A],
        ..session(Some(RoleCeiling::Member))
    };
    let ws = Resource::workspace(WS_A, ORG);
    assert!(allows(&elevated, Action::WorkspaceManage, &ws));
    for (ceiling, want) in [
        (RoleCeiling::Owner, true),
        (RoleCeiling::Admin, true),
        (RoleCeiling::Member, false),
        (RoleCeiling::Viewer, false),
    ] {
        for facts in both_ways(&elevated, &token(vec![grant(Some(WS_A), ceiling)])) {
            assert_eq!(
                allows(&facts, Action::WorkspaceManage, &ws),
                want,
                "{ceiling:?}"
            );
        }
    }
}

#[test]
fn a_per_app_admin_is_capped_by_the_ceiling_like_every_other_role() {
    // The `app_members` admin row is an admin's authority over the app, so a
    // token carries it only at an admin ceiling — exactly as it carries an org
    // admin's. It used to pass at any ceiling.
    let app = Resource::app(APP, ORG).published_from(WS_A);
    assert!(allows(&app_admin(), Action::AppAdmin, &app), "a session");
    for (ceiling, want) in [
        (RoleCeiling::Owner, true),
        (RoleCeiling::Admin, true),
        (RoleCeiling::Member, false),
        (RoleCeiling::Viewer, false),
    ] {
        for grants in [vec![grant(None, ceiling)], vec![grant(Some(WS_A), ceiling)]] {
            for facts in both_ways(&app_admin(), &token(grants)) {
                assert_eq!(allows(&facts, Action::AppAdmin, &app), want, "{ceiling:?}");
                // Capped, not locked out: the app still opens for them.
                assert!(allows(&facts, Action::AppAccess, &app), "{ceiling:?}");
            }
        }
    }
    // An unrestricted token narrows nothing here either.
    let unrestricted = app_admin().narrowed_by(&TokenReach::unrestricted());
    assert!(allows(&unrestricted, Action::AppAdmin, &app));
}

#[test]
fn an_apps_ceiling_is_the_one_over_the_workspace_it_was_published_from() {
    // Viewer on the app's workspace, admin on a sibling: the sibling's grant
    // says nothing about this app — for its per-app admin or an org officer.
    let token = token(vec![
        grant(Some(WS_A), RoleCeiling::Viewer),
        grant(Some(WS_B), RoleCeiling::Admin),
    ]);
    let app_in = |ws: Uuid| Resource::app(APP, ORG).published_from(ws);
    for bearer in [app_admin(), session(Some(RoleCeiling::Owner))] {
        assert!(allows(&bearer, Action::AppAdmin, &app_in(WS_A)));
        for facts in both_ways(&bearer, &token) {
            assert!(!allows(&facts, Action::AppAdmin, &app_in(WS_A)));
            assert!(allows(&facts, Action::AppAdmin, &app_in(WS_B)));
            for action in Action::ALL {
                assert!(
                    !allows(&facts, action, &app_in(WS_C)),
                    "{action:?} on an app of an ungranted workspace"
                );
            }
            // An app resource that names no workspace keeps the coarse answer:
            // the org's highest grant.
            assert!(allows(&facts, Action::AppAdmin, &Resource::app(APP, ORG)));
        }
    }
}

#[test]
fn a_token_with_no_grant_on_the_target_decides_like_a_non_member() {
    let non_member = session(None);
    // Granted elsewhere: another org entirely, and one other workspace here.
    let elsewhere = token(vec![
        TokenGrant {
            org_id: OTHER_ORG,
            workspace_id: None,
            ceiling: RoleCeiling::Owner,
        },
        grant(Some(WS_B), RoleCeiling::Owner),
    ]);
    let targets = [Resource::org(ORG), Resource::workspace(WS_A, ORG)];
    for role in ROLES {
        for facts in both_ways(&session(Some(role)), &elsewhere) {
            for action in Action::ALL {
                for resource in &targets {
                    assert_eq!(
                        allows(&facts, action, resource),
                        allows(&non_member, action, resource),
                        "role {role:?}, {action:?} on {:?}",
                        resource.kind
                    );
                    assert!(!allows(&facts, action, resource));
                }
            }
        }
    }
}

#[test]
fn a_token_with_no_grants_at_all_reaches_nothing() {
    // `all_access = false` with every grant gone (a deleted workspace cascades
    // its grant away) must fail closed, not fall back to the bearer's reach.
    let empty = token(Vec::new());
    for facts in both_ways(&session(Some(RoleCeiling::Owner)), &empty) {
        for action in Action::ALL {
            for resource in tenant_resources() {
                assert!(!allows(&facts, action, &resource), "{action:?}");
            }
        }
    }
}

#[test]
fn outside_its_grants_a_token_does_not_even_read_its_bearers_own_thread() {
    let mut thread = Resource::org(OTHER_ORG);
    thread.kind = crate::ResourceKind::Thread;
    thread.owner = Some(USER);
    let owner_of_thread = session(Some(RoleCeiling::Member));
    assert!(allows(&owner_of_thread, Action::OrgRead, &thread));
    for facts in both_ways(
        &owner_of_thread,
        &token(vec![grant(None, RoleCeiling::Owner)]),
    ) {
        assert!(!allows(&facts, Action::OrgRead, &thread));
    }
}

fn staff(standing: Option<PlatformStanding>, is_global_owner: bool) -> PrincipalFacts {
    PrincipalFacts {
        user_id: USER,
        platform: standing,
        is_global_owner,
        ..Default::default()
    }
}

fn staff_shapes() -> Vec<PrincipalFacts> {
    vec![
        staff(None, true),
        staff(
            Some(PlatformStanding::from_role(
                PlatformRole::GlobalAdmin,
                Scope::All,
            )),
            false,
        ),
        staff(
            Some(PlatformStanding::from_role(
                PlatformRole::AppOperator,
                Scope::Orgs(vec![ORG]),
            )),
            false,
        ),
        // Staff who is also a real member somewhere.
        PrincipalFacts {
            member_orgs: vec![ORG],
            ..staff(
                Some(PlatformStanding::from_role(
                    PlatformRole::GlobalAdmin,
                    Scope::All,
                )),
                false,
            )
        },
    ]
}

fn every_resource() -> Vec<Resource> {
    let mut all = tenant_resources();
    all.push(Resource::platform());
    all.push(Resource::partner(PARTNER));
    all.push(Resource::partner_client(ORG, PARTNER));
    all
}

#[test]
fn platform_false_decides_like_a_principal_with_no_standing() {
    let no_platform = TokenReach {
        platform: false,
        ..TokenReach::unrestricted()
    };
    for shape in staff_shapes() {
        let without = PrincipalFacts {
            platform: None,
            is_global_owner: false,
            ..shape.clone()
        };
        for facts in both_ways(&shape, &no_platform) {
            for action in Action::ALL {
                for resource in every_resource() {
                    assert_eq!(
                        allows(&facts, action, &resource),
                        allows(&without, action, &resource),
                        "{action:?} on {:?}",
                        resource.kind
                    );
                }
            }
            assert!(!facts.is_staff());
            assert!(!facts.is_root());
            assert!(!facts.is_global_admin());
            assert_eq!(facts.platform_scope(), None);
        }
    }
}

fn partner_facts() -> PrincipalFacts {
    PrincipalFacts {
        user_id: USER,
        partners: vec![PartnerStanding {
            partner_id: PARTNER,
            client_orgs: vec![ORG],
            caps: vec![
                Cap::ManageMembers,
                Cap::ManageApps,
                Cap::DevelopApps,
                Cap::CreateOrgs,
            ],
        }],
        ..Default::default()
    }
}

#[test]
fn partner_false_decides_like_a_principal_with_no_partner_standing() {
    let no_partner = TokenReach {
        partner: false,
        ..TokenReach::unrestricted()
    };
    let without = PrincipalFacts {
        partners: Vec::new(),
        ..partner_facts()
    };
    // Not vacuous: the standing does grant something to take away.
    assert!(allows(
        &partner_facts(),
        Action::PartnerManageMembers,
        &Resource::partner_client(ORG, PARTNER)
    ));
    for facts in both_ways(&partner_facts(), &no_partner) {
        for action in Action::ALL {
            for resource in every_resource() {
                assert_eq!(
                    allows(&facts, action, &resource),
                    allows(&without, action, &resource),
                    "{action:?} on {:?}",
                    resource.kind
                );
            }
        }
    }
}

#[test]
fn an_unrestricted_token_decides_exactly_as_a_session() {
    // The no-op: all-access with both standings must change no decision, for
    // any shape of principal. It is what an `oxyc login` token carries.
    let mut shapes = staff_shapes();
    shapes.push(partner_facts());
    shapes.extend(ROLES.map(|r| session(Some(r))));
    shapes.push(session(None));
    for shape in shapes {
        for facts in both_ways(&shape, &TokenReach::unrestricted()) {
            for action in Action::ALL {
                for resource in every_resource() {
                    assert_eq!(
                        allows(&facts, action, &resource),
                        allows(&shape, action, &resource),
                        "{action:?} on {:?}",
                        resource.kind
                    );
                }
            }
        }
    }
}

#[test]
fn staff_reach_through_a_token_is_capped_by_the_ceiling_like_a_membership() {
    let admin = staff(
        Some(PlatformStanding::from_role(
            PlatformRole::GlobalAdmin,
            Scope::All,
        )),
        false,
    );
    let decide = |ceiling: RoleCeiling, action: Action, resource: Resource| {
        let facts = admin
            .clone()
            .narrowed_by(&token(vec![grant(None, ceiling)]));
        allows(&facts, action, &resource)
    };
    let org = || Resource::org(ORG);
    // At `owner` the token carries the whole override into its org…
    for action in Action::ALL {
        assert_eq!(
            decide(RoleCeiling::Owner, action, org()),
            allows(&admin, action, &org()),
            "{action:?}"
        );
    }
    // …and below it, only the rings the ceiling reaches.
    assert!(decide(RoleCeiling::Member, Action::OrgRead, org()));
    assert!(!decide(RoleCeiling::Member, Action::MemberSetRole, org()));
    assert!(decide(RoleCeiling::Admin, Action::MemberSetRole, org()));
    assert!(!decide(RoleCeiling::Admin, Action::OrgOwnerManage, org()));
    // The grant is the boundary: no reach into an org it does not name.
    for action in Action::ALL {
        assert!(!decide(
            RoleCeiling::Owner,
            action,
            Resource::org(OTHER_ORG)
        ));
    }
}

#[test]
fn a_bounded_token_cannot_be_root() {
    let owner = staff(None, true);
    let bounded = token(vec![grant(None, RoleCeiling::Owner)]);
    let facts = owner.clone().narrowed_by(&bounded);
    assert!(!facts.is_global_owner, "root is unbounded by definition");
    assert!(!facts.is_root());
    assert_eq!(facts.platform_scope(), Some(&Scope::Orgs(vec![ORG])));
    assert!(!allows(
        &facts,
        Action::PlatformOwnerOnly,
        &Resource::platform()
    ));
    // It still opens the console sections a Global Admin holds, scoped.
    assert!(allows(&facts, Action::PlatformApps, &Resource::platform()));

    // Carried bare (no source cap), the model still refuses what is outside
    // the grant — root included.
    let bare = PrincipalFacts {
        token: Some(bounded),
        ..owner
    };
    assert!(!allows(&bare, Action::OrgRead, &Resource::org(OTHER_ORG)));
}

#[test]
fn a_scoped_grant_narrows_to_the_orgs_both_name() {
    let reach = token(vec![grant(None, RoleCeiling::Owner)]);
    let scoped =
        PlatformStanding::from_role(PlatformRole::AppOperator, Scope::Orgs(vec![ORG, OTHER_ORG]));
    let (root, narrowed) = reach.narrow_platform(false, Some(scoped));
    assert!(!root);
    assert_eq!(narrowed.unwrap().scope, Scope::Orgs(vec![ORG]));

    let elsewhere =
        PlatformStanding::from_role(PlatformRole::AppOperator, Scope::Orgs(vec![OTHER_ORG]));
    let (_, narrowed) = reach.narrow_platform(false, Some(elsewhere));
    assert_eq!(narrowed.unwrap().scope, Scope::Orgs(Vec::new()));
}

#[test]
fn a_bounded_partner_token_keeps_only_the_clients_it_names() {
    let two_clients = PartnerStanding {
        partner_id: PARTNER,
        client_orgs: vec![ORG, OTHER_ORG],
        caps: vec![Cap::ManageMembers, Cap::CreateOrgs],
    };
    let reach = token(vec![grant(None, RoleCeiling::Admin)]);
    let narrowed = reach.narrow_partners(vec![two_clients.clone()]);
    assert_eq!(narrowed[0].client_orgs, vec![ORG]);

    let facts = PrincipalFacts {
        user_id: USER,
        partners: vec![two_clients],
        ..Default::default()
    }
    .narrowed_by(&reach);
    let client = |org: Uuid| Resource::partner_client(org, PARTNER);
    assert!(allows(&facts, Action::PartnerManageMembers, &client(ORG)));
    assert!(!allows(
        &facts,
        Action::PartnerManageMembers,
        &client(OTHER_ORG)
    ));
    // Creating a client reaches past any list of orgs a token could name.
    assert!(!allows(
        &facts,
        Action::PartnerCreateOrgs,
        &Resource::partner(PARTNER)
    ));
}

#[test]
fn org_routes_need_an_org_wide_grant() {
    let reach = token(vec![grant(Some(WS_A), RoleCeiling::Owner)]);
    assert_eq!(reach.org_ceiling(ORG), None);
    assert_eq!(reach.workspace_ceiling(ORG, WS_A), Some(RoleCeiling::Owner));
    assert_eq!(reach.workspace_ceiling(ORG, WS_B), None);
    assert!(reach.touches_org(ORG));
    assert!(!reach.touches_org(OTHER_ORG));

    let org_wide = token(vec![grant(None, RoleCeiling::Member)]);
    assert_eq!(org_wide.org_ceiling(ORG), Some(RoleCeiling::Member));
    // "Every workspace in the org, including ones created later."
    assert_eq!(
        org_wide.workspace_ceiling(ORG, Uuid::from_u128(0xFFFF)),
        Some(RoleCeiling::Member)
    );
}

#[test]
fn the_highest_matching_grant_wins() {
    let reach = token(vec![
        grant(None, RoleCeiling::Viewer),
        grant(Some(WS_A), RoleCeiling::Admin),
    ]);
    assert_eq!(reach.workspace_ceiling(ORG, WS_A), Some(RoleCeiling::Admin));
    assert_eq!(
        reach.workspace_ceiling(ORG, WS_B),
        Some(RoleCeiling::Viewer)
    );
    assert_eq!(reach.org_ceiling(ORG), Some(RoleCeiling::Viewer));
}

#[test]
fn ceilings_are_ordered_and_round_trip() {
    assert!(RoleCeiling::Viewer < RoleCeiling::Member);
    assert!(RoleCeiling::Member < RoleCeiling::Admin);
    assert!(RoleCeiling::Admin < RoleCeiling::Owner);
    for ceiling in RoleCeiling::ALL {
        assert_eq!(RoleCeiling::parse(ceiling.as_str()), Some(ceiling));
    }
    assert_eq!(RoleCeiling::parse("root"), None);
}

#[test]
fn holding_a_standing_is_what_the_credential_carries() {
    // What a token's `platform` / `partner` flags need their owner to hold. A
    // session holds what its principal holds; a token only what it carries.
    let partner = partner_facts();
    assert!(partner.is_partner() && !partner.is_staff());
    assert!(!session(Some(RoleCeiling::Owner)).is_partner());

    let without_partner = TokenReach {
        partner: false,
        ..TokenReach::unrestricted()
    };
    assert!(!partner.clone().narrowed_by(&without_partner).is_partner());
    assert!(
        partner
            .narrowed_by(&TokenReach::unrestricted())
            .is_partner()
    );

    let without_platform = TokenReach {
        platform: false,
        ..TokenReach::unrestricted()
    };
    for staff in staff_shapes() {
        assert!(staff.is_staff());
        assert!(!staff.narrowed_by(&without_platform).is_staff());
    }
}

// ── An org ends a token's reach (the inventory's revoke-grant) ───────────────

#[test]
fn a_blocked_org_is_outside_an_all_access_token() {
    // All-access has no grant row to revoke, so the block is its own fact —
    // and it takes the org away without touching anything else.
    let blocked = TokenReach {
        blocked_orgs: vec![ORG],
        ..TokenReach::unrestricted()
    };
    // It narrows, but it is not confined to grants: it has none to be bound by.
    assert!(blocked.narrows() && blocked.blocked_somewhere());
    assert!(
        !blocked.bound(),
        "a block takes one org, not the flat routes"
    );
    let unrestricted = TokenReach::unrestricted();
    assert!(!unrestricted.bound() && !unrestricted.blocked_somewhere());
    assert_eq!(blocked.org_ceiling(ORG), None);
    assert_eq!(blocked.workspace_ceiling(ORG, WS_A), None);
    assert!(!blocked.touches_org(ORG));
    assert_eq!(blocked.org_ceiling(OTHER_ORG), Some(RoleCeiling::Owner));

    let owner = session(Some(RoleCeiling::Owner)).narrowed_by(&blocked);
    assert!(owner.member_orgs.is_empty());
    for resource in tenant_resources() {
        for action in Action::ALL {
            assert!(
                !allows(&owner, action, &resource),
                "{action:?} on {:?}",
                resource.kind
            );
        }
    }
}

#[test]
fn a_blocked_org_is_outside_a_token_whatever_its_grants_say() {
    // A live grant row beside the block reaches nothing: the org's decision
    // wins over a grant added, or left, after it.
    let blocked = TokenReach {
        blocked_orgs: vec![ORG],
        ..token(vec![grant(None, RoleCeiling::Owner)])
    };
    assert_eq!(blocked.org_ceiling(ORG), None);
    assert_eq!(blocked.ceiling_in_org(ORG), None);
    // Bound by its grants as it always was — the block is not what binds it.
    assert!(blocked.bound() && !blocked.blocked_somewhere());
    let facts = session(Some(RoleCeiling::Owner)).narrowed_by(&blocked);
    assert!(!allows(&facts, Action::OrgRead, &Resource::org(ORG)));
    // Staff standing bounded to the grants' orgs loses the blocked one too.
    let (root, standing) = blocked.narrow_platform(true, None);
    assert!(!root);
    assert_eq!(
        standing.map(|s| s.scope),
        Some(Scope::Orgs(Vec::new())),
        "a blocked org is not in the bounded scope"
    );
}
