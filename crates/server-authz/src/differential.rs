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
    Action, Cap, PartnerStanding, PlatformRole, PlatformStanding, PrincipalFacts, Resource,
    RoleCeiling, Scope, TokenGrant, TokenReach, allows,
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

/// Org API access (API-tokens design §8 Phase 3): service accounts, the org
/// token inventory, and ending a token's reach into the org.
///
/// All three handlers take `OrgAdminFor<_>`, whose shipped half is the same
/// Owner-or-Admin role check as `OrgAdmin`. Stated per action, like the
/// assignment graph's above: a later edit that moved one of them to
/// `OrgAdminStrict`, `OrgMemberStrict` or the owner ring must fail here.
#[test]
fn service_account_manage_ring_matches_the_shipped_guard() {
    // service_accounts::handlers — every route under /service-accounts
    assert_matches_oracle(Action::ServiceAccountManage, |s| {
        matches!(s.ctx_role, OrgRole::Owner | OrgRole::Admin)
    });
}

#[test]
fn token_inventory_view_ring_matches_the_shipped_guard() {
    // org_tokens::handlers::{list_org_tokens, get_org_token_activity}
    assert_matches_oracle(Action::TokenInventoryView, |s| {
        matches!(s.ctx_role, OrgRole::Owner | OrgRole::Admin)
    });
}

#[test]
fn token_grant_revoke_ring_matches_the_shipped_guard() {
    // org_tokens::handlers::revoke_org_grant
    assert_matches_oracle(Action::TokenGrantRevoke, |s| {
        matches!(s.ctx_role, OrgRole::Owner | OrgRole::Admin)
    });
}

/// Org token policy (API-tokens design §5, §8 Phase 5): `GET|PUT
/// /orgs/{org_id}/token-policy` take `OrgAdminFor<TokenPolicy>`, whose shipped
/// half is the Owner-or-Admin role check.
#[test]
fn token_policy_manage_ring_matches_the_shipped_guard() {
    // org_api_access::handlers::{get_token_policy, put_token_policy}
    assert_matches_oracle(Action::TokenPolicyManage, |s| {
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

/// `Action::ExperimentPrice` sits on the `WorkspaceEdit` ring on purpose
/// (`crates/authz/CLAUDE.md`: reuse a ring rather than inventing a synonym), and
/// the shipped gate in front of it is `role_guards::ExperimentPricer`, whose
/// legacy term is `WorkspaceEditor`'s: a real member, or an assume-session
/// override. The model may only subtract from that.
#[test]
fn experiment_price_ring_matches_the_workspace_edit_guard() {
    let ws = Resource::workspace(Uuid::from_u128(700), org());
    for s in scenarios() {
        let price = allows(&s.facts, Action::ExperimentPrice, &ws);
        let edit = allows(&s.facts, Action::WorkspaceEdit, &ws);
        assert_eq!(
            price, edit,
            "ring drift — scenario {:?}: pricing and editing disagree",
            s.name
        );
        let legacy = s.ctx_override || s.facts.member_orgs.contains(&org());
        assert!(
            !price || legacy,
            "scenario {:?}: the model allows pricing where the shipped guard would not",
            s.name
        );
    }
}

/// Creating an experiment or a design version takes `role_guards::ExperimentAuthorGuard`,
/// whose legacy term is `WorkspaceEditor`'s (a real member, or an assume-session
/// override). The action sits on the `WorkspaceEdit` ring, like pricing.
#[test]
fn experiment_author_ring_matches_the_workspace_edit_guard() {
    let ws = Resource::workspace(Uuid::from_u128(700), org());
    for s in scenarios() {
        let author = allows(&s.facts, Action::ExperimentAuthor, &ws);
        let edit = allows(&s.facts, Action::WorkspaceEdit, &ws);
        assert_eq!(author, edit, "ring drift — scenario {:?}", s.name);
        let legacy = s.ctx_override || s.facts.member_orgs.contains(&org());
        assert!(
            !author || legacy,
            "scenario {:?}: the model allows authoring where the shipped guard would not",
            s.name
        );
    }
}

/// `POST /experiments/{name}/register` takes `ExperimentRegisterGuard`, whose legacy
/// term is `WorkspaceAdmin`'s: effective workspace role Owner or Admin, or an
/// assume-session override. The action sits on `ExperimentAbort`'s ring.
#[test]
fn experiment_register_ring_matches_the_workspace_admin_guard() {
    let ws = Resource::workspace(Uuid::from_u128(700), org());
    for s in scenarios() {
        let register = allows(&s.facts, Action::ExperimentRegister, &ws);
        let abort = allows(&s.facts, Action::ExperimentAbort, &ws);
        assert_eq!(register, abort, "ring drift — scenario {:?}", s.name);
        let legacy = matches!(s.ctx_role, OrgRole::Owner | OrgRole::Admin) || s.ctx_override;
        assert!(
            !register || legacy,
            "scenario {:?}: the model allows registration where the shipped guard would not",
            s.name
        );
    }
}

/// Applying a write-back takes `role_guards::ExperimentWriteBackApplier`, whose
/// legacy term is `WorkspaceEditor`'s (a real member, or an assume-session
/// override). The action sits on the `WorkspaceEdit` ring; the model may only
/// subtract from that.
#[test]
fn experiment_apply_write_back_ring_matches_the_workspace_edit_guard() {
    let ws = Resource::workspace(Uuid::from_u128(700), org());
    for s in scenarios() {
        let apply = allows(&s.facts, Action::ExperimentApplyWriteBack, &ws);
        let edit = allows(&s.facts, Action::WorkspaceEdit, &ws);
        assert_eq!(
            apply, edit,
            "ring drift — scenario {:?}: applying a write-back and editing disagree",
            s.name
        );
        let legacy = s.ctx_override || s.facts.member_orgs.contains(&org());
        assert!(
            !apply || legacy,
            "scenario {:?}: the model allows a write-back where the shipped guard would not",
            s.name
        );
    }
}

/// `POST /experiments/{name}/abort` takes `ExperimentAbortGuard`, whose legacy
/// term is `WorkspaceAdmin`'s: effective workspace role Owner or Admin. The
/// action sits on the `WorkspaceManage` ring on purpose (it runs `lever.off`,
/// like schedule Run-now), so the model must answer exactly as it does for
/// `WorkspaceManage`, and may only subtract from the shipped guard: an
/// Owner/Admin org role, or an assume-session override.
#[test]
fn experiment_abort_ring_matches_the_workspace_admin_guard() {
    let ws = Resource::workspace(Uuid::from_u128(700), org());
    for s in scenarios() {
        let abort = allows(&s.facts, Action::ExperimentAbort, &ws);
        let manage = allows(&s.facts, Action::WorkspaceManage, &ws);
        assert_eq!(
            abort, manage,
            "ring drift — scenario {:?}: abort and workspace management disagree",
            s.name
        );
        let legacy = matches!(s.ctx_role, OrgRole::Owner | OrgRole::Admin) || s.ctx_override;
        assert!(
            !abort || legacy,
            "scenario {:?}: the model allows abort where the shipped guard would not",
            s.name
        );
    }
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
fn app_non_production_ring_matches_the_staff_draft_preview_gate() {
    // Oracle = `globals::platform_reaches(.., Cap::DevelopApps, app.org_id)`, the gate
    // the staff draft preview ships with and the `existing_allow` the staging serve
    // path passes: the global owner, or a grant holding develop_apps whose scope
    // reaches the app's org. No tenant standing: not a member, an officer, an app
    // admin, a partner or a frontline worker.
    for restricted in [false, true] {
        assert_app_ring(Action::AppNonProduction, restricted, |s| s.is_staff);
    }

    // The two axes the app scenarios hold fixed (staff there is an unscoped
    // Global Admin): scope, and whether the grant carries the capability.
    let resource = Resource::app(app_id(), org());
    let other = Uuid::from_u128(2);
    let shapes: Vec<(&str, bool, Option<PlatformStanding>, bool)> = vec![
        ("global owner", true, None, false),
        (
            "app operator scoped to this org",
            false,
            Some(PlatformStanding::from_role(
                PlatformRole::AppOperator,
                Scope::Orgs(vec![org()]),
            )),
            false,
        ),
        (
            "app operator scoped to another org",
            false,
            Some(PlatformStanding::from_role(
                PlatformRole::AppOperator,
                Scope::Orgs(vec![other]),
            )),
            false,
        ),
        (
            "app operator scoped to another org who is a member here",
            false,
            Some(PlatformStanding::from_role(
                PlatformRole::AppOperator,
                Scope::Orgs(vec![other]),
            )),
            true,
        ),
        (
            "staff without develop_apps",
            false,
            Some(PlatformStanding {
                role: PlatformRole::AppOperator,
                caps: vec![Cap::ManageApps],
                scope: Scope::All,
            }),
            false,
        ),
        ("nobody", false, None, false),
    ];
    for (name, is_global_owner, platform, is_member) in shapes {
        let expected = is_global_owner
            || platform
                .as_ref()
                .is_some_and(|g| g.grants(Cap::DevelopApps, org()));
        let facts = PrincipalFacts {
            user_id: user(),
            is_global_owner,
            platform,
            member_orgs: if is_member { vec![org()] } else { vec![] },
            ..Default::default()
        };
        assert_eq!(
            allows(&facts, Action::AppNonProduction, &resource),
            expected,
            "ring drift for AppNonProduction — scenario {name:?}"
        );
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

/// The same resolution under an API token (API-tokens design §4.4): the
/// session's verdict at an admin ceiling over the workspace the app was
/// published from, and nobody below it.
///
/// Oracle = `resolve_app_role` for a session, capped as every role is — a token
/// of ceiling `c` on a role `r` decides as a session of `min(r, c)`. The
/// scenario this pins is the per-app admin: its term carried no ceiling, so a
/// viewer token of an app admin still read `appRole: 'admin'` and passed
/// `require_app_admin`.
#[test]
fn app_admin_ring_under_a_token_is_the_shipped_resolution_capped_at_its_ceiling() {
    let resource = Resource::app_with_visibility(app_id(), org(), false).published_from(ws_id());
    for ceiling in RoleCeiling::ALL {
        // Staff standing rides along, so the staff scenario is capped by the
        // ceiling and not by a missing flag.
        let token = TokenReach {
            all_access: false,
            platform: true,
            partner: true,
            grants: vec![TokenGrant {
                org_id: org(),
                workspace_id: Some(ws_id()),
                ceiling,
            }],
            blocked_orgs: Vec::new(),
            sandbox_agent: None,
        };
        for s in app_scenarios() {
            let session = s.is_staff || s.is_org_owner || s.is_org_admin || s.is_app_admin;
            assert_eq!(
                allows(&s.facts(), Action::AppAdmin, &resource),
                session,
                "a session — scenario {:?}",
                s.name
            );
            let expected = session && ceiling >= RoleCeiling::Admin;
            let actual = allows(&s.facts().narrowed_by(&token), Action::AppAdmin, &resource);
            assert_eq!(
                actual, expected,
                "ring drift for AppAdmin under a {ceiling:?} token — scenario {:?}: the capped \
                 resolution says {expected}, the model says {actual}",
                s.name
            );
        }
    }
}

// ── Workspace previews ─────────────────────────────────────────────────────────
//
// A new surface, but not one without a shipped check: the `WorkspacePreviewer`
// guard passes the `/admin` console's staff door (`OXY_OWNER`, or any row in the
// platform-grant table) as `existing_allow`. That door is the oracle. The ring may
// only SUBTRACT from it, and it subtracts exactly two shapes the door is too coarse
// to see — both stated below rather than left to be discovered.

/// The staff door as the loader reports it: the owner allow-list or a grant row of
/// any role. What `globals::platform_standing(..).is_staff()` answers.
fn staff_door(facts: &PrincipalFacts) -> bool {
    facts.is_global_owner || facts.platform.is_some()
}

#[test]
fn workspace_preview_ring_matches_the_staff_door_across_the_org_caller_shapes() {
    let ws = Resource::workspace(ws_id(), org());
    for s in scenarios() {
        let expected = staff_door(&s.facts);
        let actual = allows(&s.facts, Action::WorkspacePreview, &ws);
        assert_eq!(
            actual, expected,
            "ring drift for WorkspacePreview — scenario {:?}: the staff door says \
             {expected}, the model says {actual}",
            s.name
        );
    }
}

#[test]
fn workspace_preview_subtracts_only_what_the_staff_door_cannot_see() {
    let ws = Resource::workspace(ws_id(), org());
    let base = || PrincipalFacts {
        user_id: user(),
        ..Default::default()
    };

    // An App Operator passes the door (it holds a grant row) but its grant is
    // custom apps and nothing else — previewing a workspace is not an app.
    let app_operator = PrincipalFacts {
        platform: Some(PlatformStanding::from_role(
            PlatformRole::AppOperator,
            Scope::All,
        )),
        ..base()
    };
    assert!(staff_door(&app_operator));
    assert!(!allows(&app_operator, Action::WorkspacePreview, &ws));

    // A Global Admin bounded to another org passes the door, which never consults
    // scope; the workspace is a tenant resource, so the ring does.
    let elsewhere = PrincipalFacts {
        platform: Some(PlatformStanding::from_role(
            PlatformRole::GlobalAdmin,
            Scope::Orgs(vec![Uuid::from_u128(2)]),
        )),
        ..base()
    };
    assert!(staff_door(&elsewhere));
    assert!(!allows(&elsewhere, Action::WorkspacePreview, &ws));

    // The same grant bounded to THIS org is not subtracted.
    let here = PrincipalFacts {
        platform: Some(PlatformStanding::from_role(
            PlatformRole::GlobalAdmin,
            Scope::Orgs(vec![org()]),
        )),
        ..base()
    };
    assert!(allows(&here, Action::WorkspacePreview, &ws));
}

// ── Workspace creation ─────────────────────────────────────────────────────────
//
// `OrgWorkspaceProvisioner` asks the org-admin door and then this ring, over the
// org. The ring is the preview ring on an org resource, so the oracle is the same
// staff door: customers (any org role, any membership) are refused, staff pass.

#[test]
fn workspace_provision_ring_matches_the_staff_door_across_the_org_caller_shapes() {
    let target = Resource::org(org());
    for s in scenarios() {
        let expected = staff_door(&s.facts);
        let actual = allows(&s.facts, Action::WorkspaceProvision, &target);
        assert_eq!(
            actual, expected,
            "ring drift for WorkspaceProvision — scenario {:?}: the staff door says \
             {expected}, the model says {actual}",
            s.name
        );
    }
}

#[test]
fn workspace_provision_refuses_a_customers_own_org_owner() {
    let target = Resource::org(org());
    let owner = PrincipalFacts {
        user_id: user(),
        owned_orgs: vec![org()],
        admin_orgs: vec![org()],
        member_orgs: vec![org()],
        ..Default::default()
    };
    assert!(!allows(&owner, Action::WorkspaceProvision, &target));
}

// ── The Factory ────────────────────────────────────────────────────────────────
//
// A new surface with NO shipped check to difference against: before this, every
// workspace member reached every authoring route. So the oracle is the rule as it
// is stated, spelled out independently of the ring over the same caller shapes —
// staff reach (an all-scope Global Admin or the Global Owner), an owner or admin of the
// org (decided 2026-10-27), or a real member of the org holding a grant for this
// workspace — and nothing else: no plain membership, no workspace role, no partner.

/// The rule, restated in plain terms over the loader's facts.
fn factory_oracle(facts: &PrincipalFacts, ws: Uuid) -> bool {
    let staff = facts.is_global_owner
        || facts
            .platform
            .as_ref()
            .is_some_and(|p| p.grants(Cap::OperatePlatform, org()));
    let officer = facts.admin_orgs.contains(&org());
    let member = facts.member_orgs.contains(&org());
    let granted = facts.factory_workspaces.contains(&ws);
    staff || officer || (member && granted)
}

#[test]
fn factory_use_ring_matches_the_stated_rule_across_the_org_caller_shapes() {
    let ws = ws_id();
    let resource = Resource::workspace(ws, org());
    for s in scenarios() {
        // Each shape twice: as the loader reports it, and with the grant added.
        for granted in [false, true] {
            let facts = PrincipalFacts {
                factory_workspaces: if granted { vec![ws] } else { Vec::new() },
                ..s.facts.clone()
            };
            let expected = factory_oracle(&facts, ws);
            let actual = allows(&facts, Action::FactoryUse, &resource);
            assert_eq!(
                actual, expected,
                "ring drift for FactoryUse — scenario {:?} (grant held: {granted}): \
                 the stated rule says {expected}, the model says {actual}",
                s.name
            );
        }
    }
}

#[test]
fn factory_use_is_refused_to_every_tenant_role_below_org_admin_without_a_grant() {
    let resource = Resource::workspace(ws_id(), org());
    for s in scenarios() {
        let staff = s.facts.is_global_owner || s.facts.platform.is_some();
        let officer = s.facts.admin_orgs.contains(&org());
        if staff || officer {
            continue;
        }
        assert!(
            !allows(&s.facts, Action::FactoryUse, &resource),
            "scenario {:?}: no plain membership, workspace override or partner standing \
             is the Factory",
            s.name
        );
    }
}

#[test]
fn factory_use_admits_the_orgs_owners_and_admins_without_a_grant() {
    let resource = Resource::workspace(ws_id(), org());
    let officers: Vec<_> = scenarios()
        .into_iter()
        .filter(|s| s.facts.admin_orgs.contains(&org()))
        .collect();
    assert!(
        officers.iter().any(|s| s.facts.owned_orgs.contains(&org())),
        "the scenarios must include a real org owner"
    );
    for s in officers {
        assert!(
            s.facts.factory_workspaces.is_empty(),
            "scenario {:?} must hold no grant",
            s.name
        );
        assert!(
            allows(&s.facts, Action::FactoryUse, &resource),
            "scenario {:?}: an owner or admin of the org reaches the Factory",
            s.name
        );
    }
}

// What `GET /orgs` reports as `may_provision_workspace` is this ring over the same
// facts, so the web app's "New workspace" control and the guard cannot disagree.
#[test]
fn provision_hint_admits_only_the_staff_the_ring_admits() {
    let here = org();
    let elsewhere = Uuid::from_u128(2);
    let staff = |role, scope| PrincipalFacts {
        user_id: user(),
        platform: Some(PlatformStanding::from_role(role, scope)),
        ..Default::default()
    };
    let hint = |facts: &PrincipalFacts| crate::provisionable(facts, &[here]).contains(&here);

    assert!(hint(&staff(PlatformRole::GlobalAdmin, Scope::All)));
    assert!(hint(&staff(
        PlatformRole::GlobalAdmin,
        Scope::Orgs(vec![here])
    )));
    // `is_app_admin` is true for these two; the ring refuses both.
    assert!(!hint(&staff(PlatformRole::AppOperator, Scope::All)));
    assert!(!hint(&staff(
        PlatformRole::GlobalAdmin,
        Scope::Orgs(vec![elsewhere])
    )));

    // Scope is per org: the same grant is admitted in one org and not another.
    let scoped = staff(PlatformRole::GlobalAdmin, Scope::Orgs(vec![here]));
    let reachable = crate::provisionable(&scoped, &[here, elsewhere]);
    assert_eq!(reachable, std::collections::HashSet::from([here]));
}

// The sandbox agent token against the same gates, over the same caller shapes.
#[path = "differential_sandbox.rs"]
mod sandbox;

// And the token's staging option: the same oracle, with staging of a granted
// app covered and everything else as it was.
#[path = "differential_sandbox_staging.rs"]
mod sandbox_staging;
