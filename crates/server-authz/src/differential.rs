//! Differential tests: the shipped legacy checks are the **oracle**, and the unified
//! model must agree with them across the whole scenario space.
//!
//! ## Why this exists instead of a production shadow window
//!
//! The plan for flipping a ring to the model-as-authority was "watch `authz.shadow` until
//! `disagree` holds at zero on real traffic". That only works if there IS real
//! traffic. With little or none, `disagree == 0` does not mean the model is right — it
//! means nobody hit it. Waiting on that is waiting on a false green.
//!
//! What actually caught every mis-modeled ring during this build (billing,
//! develop_apps, and the operator-outranks-membership escalation) was *differencing
//! the model against the legacy check*. That does not need traffic: the inputs that decide
//! an org ring are a small, finite space — the caller's real role, whether they reach
//! in via the global-operator override, whether they are Oxy staff, whether they are a
//! managing partner. So enumerate it and assert equivalence. Exhaustive instead of
//! sampled, and it runs in milliseconds with zero users.
//!
//! ## How to read a failure
//!
//! A failure means the model and the shipped check disagree for a caller shape that can
//! really occur. A disagreement is a **bug in the ring** until proven otherwise — decide
//! deliberately which side is right, and if it's the model, say so in the test rather
//! than deleting it.
//!
//! Each scenario models what the two independent paths produce for the same caller:
//! * `org_context` → the `OrgContext` the legacy guard reads (a real membership row, or
//!   a synthetic Owner/Admin with `is_global_override`).
//! * `authz::loader` → the [`PrincipalFacts`] [`allows`] reads.

use oxy_authz::{
    Action, Cap, PartnerStanding, PlatformRole, PlatformStanding, PrincipalFacts, Resource, Scope,
    allows,
};
use uuid::Uuid;

use entity::org_members::OrgRole;

/// The Global Admin preset — exactly what `is_global_admin: true` meant before staff
/// standing became a grant. Every scenario below keeps the meaning it was written with,
/// which is what makes these tests evidence that the capability split is non-breaking
/// rather than a rewrite of the oracle.
fn global_admin() -> PlatformStanding {
    PlatformStanding::from_role(PlatformRole::GlobalAdmin, Scope::All)
}

fn org() -> Uuid {
    Uuid::from_u128(1)
}
fn user() -> Uuid {
    Uuid::from_u128(100)
}

/// One caller shape, and what each path derives for it.
struct Scenario {
    name: &'static str,
    /// What `org_context` puts in `ctx.membership.role` (real row, or synthetic).
    ctx_role: OrgRole,
    /// What `org_context` sets for `is_global_override` (synthetic membership).
    ctx_override: bool,
    /// What the loader produces.
    facts: PrincipalFacts,
}

fn scenarios() -> Vec<Scenario> {
    let base = || PrincipalFacts {
        user_id: user(),
        ..Default::default()
    };
    vec![
        Scenario {
            name: "real org owner",
            ctx_role: OrgRole::Owner,
            ctx_override: false,
            facts: PrincipalFacts {
                owned_orgs: vec![org()],
                admin_orgs: vec![org()],
                member_orgs: vec![org()],
                ..base()
            },
        },
        Scenario {
            name: "real org admin",
            ctx_role: OrgRole::Admin,
            ctx_override: false,
            facts: PrincipalFacts {
                admin_orgs: vec![org()],
                member_orgs: vec![org()],
                ..base()
            },
        },
        Scenario {
            name: "real org member",
            ctx_role: OrgRole::Member,
            ctx_override: false,
            facts: PrincipalFacts {
                member_orgs: vec![org()],
                ..base()
            },
        },
        Scenario {
            // Staff with a live assume session, NOT a member: org_context synthesizes
            // Owner (assume.rs `ActingAs::Staff`).
            name: "global admin via override (not a member)",
            ctx_role: OrgRole::Owner,
            ctx_override: true,
            facts: PrincipalFacts {
                platform: Some(global_admin()),
                ..base()
            },
        },
        Scenario {
            name: "global owner via override (not a member)",
            ctx_role: OrgRole::Owner,
            ctx_override: true,
            facts: PrincipalFacts {
                is_global_owner: true,
                ..base()
            },
        },
        Scenario {
            // The escalation case: staff who is ALSO a plain member. org_context hands
            // back their REAL row, so their real role governs — the global flag must
            // not out-rank it.
            name: "global admin who is a plain member",
            ctx_role: OrgRole::Member,
            ctx_override: false,
            facts: PrincipalFacts {
                member_orgs: vec![org()],
                platform: Some(global_admin()),
                ..base()
            },
        },
        Scenario {
            name: "global admin who is a real org admin",
            ctx_role: OrgRole::Admin,
            ctx_override: false,
            facts: PrincipalFacts {
                admin_orgs: vec![org()],
                member_orgs: vec![org()],
                platform: Some(global_admin()),
                ..base()
            },
        },
        Scenario {
            // A partner operating a client: assume.rs `ActingAs::Partner` -> Admin.
            name: "managing partner via override (not a member)",
            ctx_role: OrgRole::Admin,
            ctx_override: true,
            facts: PrincipalFacts {
                partners: vec![PartnerStanding {
                    partner_id: Uuid::from_u128(900),
                    client_orgs: vec![org()],
                    caps: vec![Cap::ManageMembers],
                }],
                ..base()
            },
        },
    ]
}

/// Assert the model matches the oracle for `action` across every scenario.
fn assert_matches_oracle(action: Action, oracle: impl Fn(&Scenario) -> bool) {
    for s in scenarios() {
        let expected = oracle(&s);
        let actual = allows(&s.facts, action, &Resource::org(org()));
        assert_eq!(
            actual, expected,
            "ring drift for {action:?} — scenario {:?}: the shipped check says {expected}, \
             the model says {actual}",
            s.name
        );
    }
}

#[test]
fn org_admin_ring_matches_the_shipped_guard() {
    // role_guards::OrgAdmin
    assert_matches_oracle(Action::MemberSetRole, |s| {
        matches!(s.ctx_role, OrgRole::Owner | OrgRole::Admin)
    });
}

/// The assignment graph's two shape-changing actions.
///
/// `crates/authz/CLAUDE.md` requires a differential case per new `Action`, and
/// these arrived without one — so the ring they were given had never been
/// checked against the guard that actually ships in front of them. Both handlers
/// take `OrgAdmin`, so both must agree with that guard's oracle exactly.
///
/// Stated per action rather than looped with `MemberSetRole`, because the point
/// is that THESE actions are on the ring their routes claim: a future edit that
/// moved one to `OrgAdminStrict` or `OrgMemberStrict` would still pass a shared
/// loop over whatever ring it landed on.
#[test]
fn manage_locations_ring_matches_the_shipped_guard() {
    // work::handlers::create_location — OrgAdmin
    assert_matches_oracle(Action::ManageLocations, |s| {
        matches!(s.ctx_role, OrgRole::Owner | OrgRole::Admin)
    });
}

#[test]
fn manage_org_roles_ring_matches_the_shipped_guard() {
    // work::handlers::create_role — OrgAdmin
    assert_matches_oracle(Action::ManageOrgRoles, |s| {
        matches!(s.ctx_role, OrgRole::Owner | OrgRole::Admin)
    });
}

/// The document model's write action.
///
/// Written BEFORE the routes it will guard, which is the unusual part and the
/// point. The assignment graph's two actions arrived with no differential case,
/// so the ring they were given had never been checked against the guard in
/// front of them; this inverts that order. The oracle below IS the contract the
/// manage handlers must meet.
///
/// It does NOT check where they are mounted — this asserts the model against a
/// hand-written oracle and never reads a router or a signature, so swapping a
/// handler's extractor leaves it green. That half is
/// `every_org_scoped_document_write_takes_the_orgadmin_extractor`, in
/// `crates/app/tests/authz/document_write_guards.rs`. An earlier version of
/// this comment claimed both, and three comments in `crates/app` repeated the
/// claim, so the safety net was cited four times and existed nowhere.
///
/// Only the write side is differenced, because only the write side is a ring.
/// Reads are a query filter with no legacy check to difference against, and
/// pretending otherwise would be a test that passes for the wrong reason. The
/// cross-location read gates live with the routes, in
/// `crates/app/tests/platform/documents.rs`.
#[test]
fn manage_documents_ring_matches_the_shipped_guard() {
    // documents::handlers manage routes — OrgAdmin, same extractor as
    // create_location and create_role.
    assert_matches_oracle(Action::ManageDocuments, |s| {
        matches!(s.ctx_role, OrgRole::Owner | OrgRole::Admin)
    });
}

#[test]
fn manage_assignments_ring_matches_the_shipped_guard() {
    // operating_graph::assignments::create / remove — OrgAdmin
    assert_matches_oracle(Action::ManageAssignments, |s| {
        matches!(s.ctx_role, OrgRole::Owner | OrgRole::Admin)
    });
}

#[test]
fn org_admin_strict_ring_matches_the_shipped_guard() {
    // role_guards::OrgAdminStrict — rejects the override.
    assert_matches_oracle(Action::OrgBilling, |s| {
        !s.ctx_override && matches!(s.ctx_role, OrgRole::Owner | OrgRole::Admin)
    });
}

#[test]
fn org_owner_ring_matches_the_shipped_guard() {
    // role_guards::OrgOwner
    assert_matches_oracle(Action::OrgOwnerManage, |s| s.ctx_role == OrgRole::Owner);
}

#[test]
fn org_member_strict_ring_matches_the_shipped_guard() {
    // role_guards::OrgMemberStrict — any real member, override rejected.
    assert_matches_oracle(Action::OrgReadStrict, |s| !s.ctx_override);
}

// ── App rings ─────────────────────────────────────────────────────────────────
//
// The app actions turn on an axis the org scenarios above don't carry — per-app
// membership — and target an `App` resource that also carries its visibility. So
// they get their own scenario set rather than being bolted onto `scenarios()`.

fn app_id() -> Uuid {
    Uuid::from_u128(950)
}

/// The workspace `app_id()` was published from — what the data-plane gate is
/// keyed on, and the only id it has.
fn ws_id() -> Uuid {
    Uuid::from_u128(951)
}

/// One caller shape for the app rings, stated as the raw facts the shipped
/// checks read, so the oracle below can be written in those terms directly.
struct AppScenario {
    name: &'static str,
    is_staff: bool,
    is_org_owner: bool,
    is_org_admin: bool,
    is_org_member: bool,
    is_app_member: bool,
    is_app_admin: bool,
    has_develop_apps: bool,
    has_manage_apps: bool,
    /// An active `org_frontline_members` row — and, by construction, NO
    /// `org_members` row: `enroll_worker` mints a mailbox-less user that no
    /// email-keyed path can ever make a member.
    is_frontline: bool,
}

impl AppScenario {
    fn facts(&self) -> PrincipalFacts {
        // Owner implies admin implies member, mirroring `derive_org_roles`.
        let org_set = |include: bool| if include { vec![org()] } else { vec![] };
        let app_set = |include: bool| if include { vec![app_id()] } else { vec![] };
        PrincipalFacts {
            user_id: user(),
            owned_orgs: org_set(self.is_org_owner),
            admin_orgs: org_set(self.is_org_owner || self.is_org_admin),
            member_orgs: org_set(self.is_org_member || self.is_org_admin || self.is_org_owner),
            app_memberships: app_set(self.is_app_member || self.is_app_admin),
            app_admin_memberships: app_set(self.is_app_admin),
            frontline_orgs: org_set(self.is_frontline),
            // As the loader derives it: the workspaces of the apps the worker
            // holds a grant on, loaded only for a principal with standing.
            frontline_workspace_grants: if self.is_frontline
                && (self.is_app_member || self.is_app_admin)
            {
                vec![ws_id()]
            } else {
                vec![]
            },
            partners: {
                let mut caps = Vec::new();
                if self.has_develop_apps {
                    caps.push(Cap::DevelopApps);
                }
                if self.has_manage_apps {
                    caps.push(Cap::ManageApps);
                }
                if caps.is_empty() {
                    vec![]
                } else {
                    vec![PartnerStanding {
                        partner_id: Uuid::from_u128(900),
                        client_orgs: vec![org()],
                        caps,
                    }]
                }
            },
            platform: self.is_staff.then(global_admin),
            ..Default::default()
        }
    }
}

fn app_scenarios() -> Vec<AppScenario> {
    let base = AppScenario {
        name: "",
        is_staff: false,
        is_org_owner: false,
        is_org_admin: false,
        is_org_member: false,
        is_app_member: false,
        is_app_admin: false,
        has_develop_apps: false,
        has_manage_apps: false,
        is_frontline: false,
    };
    vec![
        AppScenario {
            name: "plain org member, no app membership",
            is_org_member: true,
            ..base
        },
        AppScenario {
            name: "org member who is an app member",
            is_org_member: true,
            is_app_member: true,
            ..base
        },
        AppScenario {
            name: "org member who is an app ADMIN",
            is_org_member: true,
            is_app_admin: true,
            ..base
        },
        AppScenario {
            name: "org admin (break-glass)",
            is_org_admin: true,
            ..base
        },
        AppScenario {
            name: "org owner (break-glass)",
            is_org_owner: true,
            ..base
        },
        AppScenario {
            name: "oxy staff (break-glass)",
            is_staff: true,
            ..base
        },
        AppScenario {
            name: "develop_apps partner, not a member",
            has_develop_apps: true,
            ..base
        },
        AppScenario {
            name: "manage_apps partner, not a member",
            has_manage_apps: true,
            ..base
        },
        // The scenario that pins the closed gap: a grant row exists but the holder
        // is NOT an org member. The serve gate used to admit them while the data
        // plane refused, so the app's shell loaded and every query 403'd. Both the
        // model and the shipped gate must now deny.
        AppScenario {
            name: "app grant WITHOUT org membership",
            is_app_member: true,
            ..base
        },
        AppScenario {
            name: "app ADMIN grant WITHOUT org membership",
            is_app_admin: true,
            ..base
        },
        // The crew. A worker enters ONLY through a grant — the two "no grant"
        // shapes are what keep an org-visible app from reaching every worker
        // on a store's roster the day somebody flips it org-wide.
        AppScenario {
            name: "frontline worker granted this app",
            is_frontline: true,
            is_app_member: true,
            ..base
        },
        AppScenario {
            name: "frontline worker granted this app as its ADMIN",
            is_frontline: true,
            is_app_admin: true,
            ..base
        },
        AppScenario {
            name: "frontline worker, no grant on this app",
            is_frontline: true,
            ..base
        },
        AppScenario {
            name: "outsider",
            ..base
        },
    ]
}

/// The oracle every frontline scenario reduces to, in both shipped gates: active
/// standing AND an explicit grant on the app (`user_can_access_app`), or on an
/// app published from the workspace (`frontline_worker_with_app_grant`) — which
/// for a fixture with one app in one workspace is the same row.
fn frontline_with_grant(s: &AppScenario) -> bool {
    s.is_frontline && (s.is_app_member || s.is_app_admin)
}

fn assert_app_ring(action: Action, restricted: bool, oracle: impl Fn(&AppScenario) -> bool) {
    let resource = Resource::app_with_visibility(app_id(), org(), restricted);
    for s in app_scenarios() {
        let expected = oracle(&s);
        let actual = allows(&s.facts(), action, &resource);
        assert_eq!(
            actual, expected,
            "ring drift for {action:?} (restricted={restricted}) — scenario {:?}: the shipped \
             check says {expected}, the model says {actual}",
            s.name
        );
    }
}

#[test]
fn app_access_ring_matches_the_shipped_gate_for_an_open_app() {
    // Oracle = the shipped decision composed: `user_can_access_app`'s staff
    // short-circuit and org-membership customer path, plus the develop_apps
    // partner term that `check_custom_app_gates` contributes. Unrestricted is
    // today's behavior and must be unchanged by the visibility work.
    assert_app_ring(Action::AppAccess, false, |s| {
        s.is_staff
            || s.has_develop_apps
            || s.is_org_member
            || s.is_org_admin
            || s.is_org_owner
            || frontline_with_grant(s)
    });
}

#[test]
fn app_access_ring_matches_the_shipped_gate_for_a_restricted_app() {
    // Same gate, `visibility = 'members'` branch: plain org membership no longer
    // suffices — a grant does, and any org officer (owner/admin) or staff stay
    // break-glass.
    //
    // The grant term is ANDed with org membership, matching `user_can_access_app`.
    // A grant NARROWS the org; it is not a way into one. The two
    // "…WITHOUT org membership" scenarios are what hold that line.
    assert_app_ring(Action::AppAccess, true, |s| {
        s.is_staff
            || s.has_develop_apps
            || s.is_org_owner
            || s.is_org_admin
            || (s.is_org_member && (s.is_app_member || s.is_app_admin))
            || frontline_with_grant(s)
    });
}

#[test]
fn workspace_data_ring_matches_the_shipped_data_plane_gate() {
    // Oracle = `check_custom_app_gates` composed: `is_org_member`, the
    // develop_apps staff and partner terms, and `frontline_worker_with_app_grant`
    // — active standing AND a grant on an app published from THIS workspace.
    //
    // The resource is the workspace, stated as one. This gate used to hand the
    // ring `Resource::app(project_id, …)` with a workspace id in the app slot, so
    // the ring could not see a worker's grant and the gate skipped `enforce` for
    // them. The three frontline scenarios are the ones that exemption covered;
    // they now go through the model like everyone else, and this is what holds
    // the model to the gate.
    let resource = Resource::workspace(ws_id(), org());
    for s in app_scenarios() {
        let expected = s.is_staff
            || s.has_develop_apps
            || s.is_org_member
            || s.is_org_admin
            || s.is_org_owner
            || frontline_with_grant(&s);
        let actual = allows(&s.facts(), Action::WorkspaceDataAccess, &resource);
        assert_eq!(
            actual, expected,
            "ring drift for WorkspaceDataAccess — scenario {:?}: the shipped gate says \
             {expected}, the model says {actual}",
            s.name
        );
    }

    // And the workspace next door, where none of these apps was published:
    // every worker is out, every member is still in. The grant is per workspace.
    let next_door = Resource::workspace(Uuid::from_u128(952), org());
    for s in app_scenarios() {
        let expected =
            s.is_staff || s.has_develop_apps || s.is_org_member || s.is_org_admin || s.is_org_owner;
        assert_eq!(
            allows(&s.facts(), Action::WorkspaceDataAccess, &next_door),
            expected,
            "scenario {:?} at a workspace holding none of their grants",
            s.name
        );
    }
}

#[test]
fn app_access_manage_ring_matches_the_shipped_grant_surface() {
    // Oracle = the teams/grants handlers' gate: an org officer (owner/admin), Oxy
    // staff, or a `manage_apps` partner. Visibility is irrelevant — who may RESTAFF
    // an app doesn't change with whether it's currently restricted.
    //
    // The two terms this test exists to pin, because they point in opposite
    // directions from `AppAdmin`: a `manage_apps` partner IS admitted here (naming an
    // audience is lifecycle) and an app-admin row is NOT (running an app's privileged
    // surface is not deciding who reaches it). A `develop_apps`-only partner is also
    // denied — the manage/develop split holds in both directions.
    for restricted in [false, true] {
        assert_app_ring(Action::AppAccessManage, restricted, |s| {
            s.is_staff || s.is_org_owner || s.is_org_admin || s.has_manage_apps
        });
    }
}

#[test]
fn app_admin_ring_matches_the_shipped_role_resolution() {
    // Oracle = `custom_apps_auth::resolve_app_role`'s "admin" verdict: any org
    // officer (owner/admin), an app-admin row, or staff. NOT a plain org member,
    // and NOT a develop_apps partner. Visibility does not affect who administers.
    for restricted in [false, true] {
        assert_app_ring(Action::AppAdmin, restricted, |s| {
            s.is_staff || s.is_org_owner || s.is_org_admin || s.is_app_admin
        });
    }
}
