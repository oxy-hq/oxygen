//! `oxy-authz` — Oxy's **authorization** decision layer: one place that states who may
//! do what, so the answer is not re-derived at ~170 call sites. Authentication (who you
//! are) lives in `oxy-auth`; this is authorization (what you may do).
//!
//! Design & rejected-engines rationale: this crate's `CLAUDE.md` (the original
//! unification design doc was distilled there; its history is in git).
//!
//! ## The model
//!
//! An [`Action`] is the closed vocabulary Oxy owns. Each maps to the authority `Ring`
//! that may perform it, and [`allows`] is the single `match` that states every ring.
//! Callers supply only FACTS — [`PrincipalFacts`], which orgs the principal
//! owns/admins/manages and the two global flags — plus a [`Resource`]. Loading those
//! facts is NOT here: it reaches app-specific primitives (org membership, partner
//! scope, the global-admin table), so it lives in `oxy-app`. This crate stays
//! transport-agnostic: it returns a bool or a [`Denied`], never an HTTP status.
//!
//! ## Why there is no policy engine
//!
//! This was built on Cedar and the engine was removed, because it earned nothing here:
//!
//! * The policy was generated from [`Action`] by string concatenation and parsed at
//!   runtime, so the one declarative, readable policy it was adopted for never existed
//!   as an artifact — what a reviewer read was Rust assembling policy text.
//! * The entity graph was degenerate. Every [`Resource`] carries its `org_id`, so the
//!   hierarchy only rediscovered what the caller had already supplied; every rule
//!   reduced to set containment, which is what [`allows`] does directly.
//! * `cedar validate` could not see the bug class this model actually hit: with every
//!   set typed `Set<Org>`, wiring the wrong capability into a ring type-checked clean.
//!   Verified — swapping `manage_apps_orgs` for `develop_apps_orgs` passed validation
//!   and was caught only by the behavioural tests.
//! * Policy-as-data (external authors, per-tenant roles) is an explicit non-goal
//!   (design §2) — and that is the requirement that pays for an engine.
//!
//! The value was never the engine: it was stating the model explicitly and DIFFERENCING
//! it against the shipped checks. That found every real bug here (billing excluded the
//! override; `develop_apps` is not `manage_apps`; operator reach must not out-rank a
//! real membership). Those tests survive; the runtime did not. Adopt an engine when
//! policy must be authored as data — not to compute `contains`.
//!
//! Fails **closed**: an unmapped combination simply has no arm that grants.

// A duplicate match arm in `allows` would silently shadow a later ring, so the
// exhaustiveness guarantee this crate rests on gets a hard lint rather than the
// default warning. Verified clean at the time of adding.
#![deny(unreachable_patterns)]

use uuid::Uuid;

/// An action a principal may take on a resource. Oxy owns this closed vocabulary, and
/// [`allows`] must have an arm for every variant — adding one here without giving it a
/// `Ring` fails the build, which is the exhaustiveness guarantee this enum exists for.
/// Grow it as families migrate.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Read anything in an org — any member.
    OrgRead,
    /// Invite / set-role / remove a member — org owner/admin, a managing partner,
    /// or global admin/owner.
    MemberInvite,
    MemberSetRole,
    MemberRemove,
    /// Create / rename / retire a LOCATION, and define the org's own roles —
    /// org owner/admin, a managing partner, or a global operator. Same ring as
    /// member management because it is the same authority: deciding the shape
    /// of the org rather than doing work inside it.
    ///
    /// Note what this is NOT. A tenant-defined role is a label and a routing
    /// target, never an authorization principal — `org_roles` grants nothing on
    /// its own. Giving it a ring of its own would invite exactly the
    /// conflation this comment exists to prevent.
    ManageLocations,
    ManageOrgRoles,
    /// Create, edit, publish, re-file or trash a DOCUMENT or folder, and decide
    /// its visibility — org owner/admin, a managing partner, or a global
    /// operator. Same ring as member and location management, and for the same
    /// reason: deciding what the org publishes to itself is deciding the shape
    /// of the org, not doing work inside it.
    ///
    /// Note the asymmetry with reading, which is the whole design. This action
    /// gates WRITES only. Who may READ a document is a query filter — a
    /// frontline worker holds no `org_members` row by design and the readable
    /// set is unbounded per user, so a ring would need an unbounded fact on the
    /// hot path. `visible_documents()` owns that; this owns nothing about it.
    ///
    /// Deliberately NOT open to a plain member in v1. Broadening authorship is
    /// a later policy knob and a safe one — the model can only subtract, so
    /// widening later cannot open a hole, while pre-widening now could.
    ManageDocuments,
    /// Put a person in a position at a place, or take them out of one —
    /// `org_role_members`. Same ring again: an assignment decides the shape of
    /// the org's roster, and a store manager holding a position is not thereby
    /// able to hand one to somebody else. Note the grantee side is NOT decided
    /// here — an assignment names a member or an active frontline worker, and
    /// the writer checks that standing the way the app access settings do.
    ManageAssignments,
    /// Billing (Stripe portal / invoices / checkout) — a **real** org owner or
    /// admin. Mirrors the `OrgAdminStrict` guard: unlike member management, the
    /// cross-tenant global-operator override does NOT reach it (Oxy staff are
    /// deliberately barred from a tenant's billing), and neither does a partner.
    OrgBilling,
    /// Owner-exclusive org operations (delete, ownership transfer, owner-promotion)
    /// — the `OrgOwner` guard: a real org **owner**, or a global operator acting in
    /// via the synthetic-Owner override. A partner (assumes only Admin) cannot.
    OrgOwnerManage,
    /// A **real** member's read (billing-status banner, checkout verify) — the
    /// `OrgMemberStrict` guard: any real org member, but NOT the cross-tenant
    /// override (so a tenant's billing state never leaks to an operator).
    OrgReadStrict,
    /// Destructive / settings-changing workspace action (delete, settings,
    /// force-push) — the `WorkspaceAdmin` guard: effective workspace Owner/Admin.
    /// Org owner/admin reach it through the org→workspace hierarchy; a per-workspace
    /// override can elevate a plain member; a managing partner (assumes Admin) and a
    /// global operator reach it via the override path.
    WorkspaceManage,
    /// Edit workspace contents (commit / push / pull / file edit) — the
    /// `WorkspaceEditor` guard: anyone above Viewer. Every org member resolves to at
    /// least workspace Member (overrides only elevate), so this is effectively any
    /// member of the workspace's org, plus a managing partner or global operator.
    WorkspaceEdit,
    /// Open a custom app, **by app id** (`user_can_access_app` — every surface that
    /// has the `apps` row in hand): a real member of the app's org, an Oxy global
    /// admin, or a partner operator whose ceiling grants `develop_apps`.
    /// Deliberately NOT the general "manage" partner path — develop_apps is the
    /// read-the-app's-data capability, distinct from manage_apps.
    ///
    /// When the app is **restricted** (`apps.visibility = 'members'`), plain org
    /// membership no longer suffices — the principal must hold an `app_members`
    /// row, or be an org officer (owner/admin) / staff / a develop_apps partner.
    /// That is the one place in this model where a fact SUBTRACTS reach rather
    /// than adding it.
    ///
    /// The workspace-keyed data plane behind an app is [`Self::WorkspaceDataAccess`],
    /// a different resource: do not hand this ring a workspace id.
    AppAccess,
    /// Reach a workspace's custom-app **data plane** (`check_custom_app_gates`):
    /// every route under `/customer-apps/{workspace}/…` — SQL and semantic queries,
    /// agent asks, automation runs, threads, activity. Keyed on a WORKSPACE, not an
    /// app, because what it serves is workspace data shared by every app published
    /// from that workspace; there is no app id on the wire to restrict by.
    ///
    /// Same reach as [`Self::AppAccess`] on an unrestricted app — an org member,
    /// Oxy staff with `develop_apps`, a `develop_apps` partner — plus a **frontline
    /// worker** holding a grant on ANY app published from this workspace. That last
    /// term is why this is its own action rather than `AppAccess` with a workspace
    /// id: the frontline grant is per app, and a ring asked about a workspace could
    /// never find it. (It was asked exactly that for a while, and the gate grew an
    /// exemption that skipped the model for workers. This is the model's answer.)
    WorkspaceDataAccess,
    /// Administer a custom app from *inside* the app — its privileged surface
    /// (e.g. the warehouse app's `?view=admin` and the data behind it). Any org
    /// **officer** (owner or admin), an `app_members` row with `role = 'admin'`,
    /// or Oxy staff.
    ///
    /// The `app_members` admin role extends admin rights DOWNWARD: it names an
    /// app's administrator who is NOT an org officer (the warehouse admin who
    /// isn't org staff), without granting org-wide billing/member management.
    /// Deliberately not a develop_apps partner — building an app is not
    /// administering its live privileged surface.
    AppAdmin,
    /// Decide WHO may open a custom app: flip `apps.visibility`, and grant or
    /// revoke `app_members` / `app_team_grants` rows. Also covers the org's team
    /// roster (`org_teams`, `org_team_members`), which exists only to be granted.
    ///
    /// An org **officer** (owner or admin), Oxy staff, or a partner whose ceiling
    /// grants `manage_apps`. Naming an app's audience is app LIFECYCLE — the same
    /// thing `manage_apps` already means — which is why the partner term belongs
    /// here and pointedly not on [`Action::AppAdmin`].
    ///
    /// Separate from [`Action::AppAdmin`] on purpose: administering an app's
    /// privileged surface (what an app admin does) is not the same authority as
    /// deciding who reaches the app at all (what an org officer does). Fusing them
    /// would either hand app admins the roster or hand partners the app's admin
    /// surface — both wrong, in opposite directions.
    ///
    /// Note this DOES let a `manage_apps` partner edit the client's team roster.
    /// That is intended — a partner curating audiences for apps it manages needs
    /// the audiences — but it is reach into org membership data, so if teams ever
    /// gate something other than apps, that consumer needs its own action rather
    /// than riding this one.
    AppAccessManage,
    /// Rename a workspace (`ensure_org_admin_or_workspace_creator`): an org
    /// owner/admin, OR the plain member who CREATED that workspace. The creator half
    /// is the only place a `created_by` self-claim grants a workspace action, so it
    /// gets its own ring rather than widening the general self rule.
    WorkspaceRename,
    /// Delete a git namespace (`github::namespaces`): an org owner/admin, OR the
    /// member who created it. Same shape as [`Action::WorkspaceRename`] — org admin or the
    /// creator — which is why they share a ring instead of each re-deriving it.
    NamespaceDelete,
    /// Flip a workspace's Oxy-access switch (`workspace_oxy_access`): a **real**
    /// workspace owner/admin. The global-operator override is rejected on purpose —
    /// staff must not be able to self-grant access to a tenant's workspace.
    WorkspaceOxyAccess,

    // ── Partner ceiling (one action per PartnerCapability) ────────────────────
    // The partner tier shipped a whole second decision engine of its own. These absorb
    // it so there is ONE place that states the entire authority model. Each is "the
    // partner I am acting as holds <capability> over this client".
    PartnerManageMembers,
    PartnerManageApps,
    PartnerDevelopApps,
    PartnerViewAudit,
    PartnerManageBilling,
    PartnerManageSecrets,
    PartnerCreateOrgs,
    PartnerManageOrgSettings,

    // ── Platform (Oxy's own operator surfaces) ────────────────────────────────
    // These are not org-scoped: they target the `Platform` singleton, so no org set and
    // no grant SCOPE is consulted — a scoped operator passes the door and the handler
    // filters the rows. Each names the capability its surface is actually about, which
    // is what lets one console serve several staff roles.
    /// The staff console **door** (`oxy_owner_or_app_admin_guard`): holds any platform
    /// standing at all. This is the outer `/admin/*` nest and nothing more — passing it
    /// means "you are staff", not "you may use this section". Every section escalates
    /// to its own capability action below, the same way billing already escalates via
    /// `route_layer`.
    PlatformOps,
    /// The custom-app registry and its publish tokens (`/admin/apps`,
    /// `/admin/app-publish-tokens`, `/customer-apps`) — [`Cap::ManageApps`].
    PlatformApps,
    /// The cross-tenant explorer (`/admin/explorer`) — reading tenants' threads and
    /// state as staff. [`Cap::ViewTenants`].
    PlatformExplorer,
    /// The staff audit log (`/admin/audit`) — [`Cap::ViewAudit`].
    PlatformAudit,
    /// Org administration: settings, subdomains, workspace administration, deletion
    /// (`/admin/orgs`, `/admin/org-subdomains`, `/admin/workspaces`) —
    /// [`Cap::ManageOrgSettings`].
    PlatformOrgs,
    /// Creating a tenant org (`POST /admin/orgs`) — [`Cap::CreateOrgs`]. Split from
    /// [`Action::PlatformOrgs`] because creating a tenant and being able to delete one
    /// are different powers, and the partner tier already draws the line there.
    PlatformOrgCreate,
    /// Staff user administration (`/admin/users`) — [`Cap::ManageMembers`].
    PlatformUsers,
    /// The partner registry (`/admin/partners`) — [`Cap::ManagePartners`].
    PlatformPartners,
    /// Oxy's own machinery: internal jobs, compiles, serve routing, platform metrics,
    /// workspace health — [`Cap::OperatePlatform`].
    PlatformOperate,
    /// Provision and inspect an org's OLTP Postgres from the admin console —
    /// [`Cap::OperatePlatform`].
    ///
    /// Deliberately NOT [`Cap::ManageApps`]: provisioning creates a billable
    /// database at the provider, so an App Operator — who ships apps and
    /// nothing else — must not reach it.
    ///
    /// Reading and provisioning share one action because the read already
    /// exposes the host and the schema layout; splitting them would imply a
    /// tier that can see a tenant's database shape but not act on it, which is
    /// not a distinction anyone has asked for.
    PlatformOltp,
    /// Staff provisioning a tenant's **Airhouse warehouse** from the admin
    /// console.
    ///
    /// The same ring as [`Self::PlatformOltp`] — both are "operate the
    /// platform's data plane on a tenant's behalf", and a grant that could
    /// create one but not the other would need a story for why. A separate
    /// *action* even so, because the action name is what lands in an audit row
    /// and in the grant UI: riding `PlatformOltp` recorded an Airhouse act
    /// under OLTP's name. Same authority, honest vocabulary.
    PlatformAirhouse,
    /// A partner provisioning OLTP for one of its client orgs —
    /// [`Cap::ManageOrgSettings`], the same capability that already covers
    /// changing what a client org is configured with.
    ///
    /// Separate from [`Self::PlatformOltp`] because the two are asked of
    /// different resources: a platform action is pinned to the Platform
    /// singleton and filtered by scope, a partner action names the partner and
    /// the client org. That is why there is no single "staff or partner" ring.
    ///
    /// **Modeled ahead of its routes, deliberately.** No handler takes this
    /// yet — partner-scoped OLTP management is not wired — but the ring and its
    /// differential cases exist so the authorization is settled before the
    /// surface ships, not bolted on after. An unrouted action is inert: nothing
    /// grants it, so it cannot widen access. When the partner OLTP routes land,
    /// they take this via `enforce_guard`, not a hand-rolled check.
    PartnerManageOltp,
    /// The grant table (`/admin/app-admins`) — [`Cap::ManagePlatformGrants`].
    ///
    /// **This action is a door, not a decision.** It answers "may this principal
    /// administer grants at all", which is necessary and nowhere near sufficient: the
    /// question that actually matters is whether they may administer *this* grant, and
    /// that compares two standings, so no `Action` can express it. The handler must
    /// also pass the target row through [`may_delegate`]. Gating the route and stopping
    /// there re-opens exactly the escalation the owner-only guard existed to prevent.
    ///
    /// The same two-step as scope: the capability gates the verb, the row-level fence
    /// filters the rows.
    PlatformGrants,
    /// The owner-exclusive surfaces — and the ONLY place the two operator tiers differ:
    /// destructive or irreversible operations (deleting the master org, demoting other
    /// admins), plus the Billing queue. A global **owner** only.
    PlatformOwnerOnly,
}

impl Action {
    pub const ALL: [Action; 42] = [
        Action::OrgRead,
        Action::ManageLocations,
        Action::ManageOrgRoles,
        Action::ManageDocuments,
        Action::ManageAssignments,
        Action::MemberInvite,
        Action::MemberSetRole,
        Action::MemberRemove,
        Action::OrgBilling,
        Action::OrgOwnerManage,
        Action::OrgReadStrict,
        Action::WorkspaceManage,
        Action::WorkspaceEdit,
        Action::AppAccess,
        Action::WorkspaceDataAccess,
        Action::AppAdmin,
        Action::AppAccessManage,
        Action::WorkspaceRename,
        Action::NamespaceDelete,
        Action::WorkspaceOxyAccess,
        Action::PartnerManageMembers,
        Action::PartnerManageApps,
        Action::PartnerDevelopApps,
        Action::PartnerViewAudit,
        Action::PartnerManageBilling,
        Action::PartnerManageSecrets,
        Action::PartnerCreateOrgs,
        Action::PartnerManageOrgSettings,
        Action::PartnerManageOltp,
        Action::PlatformOltp,
        Action::PlatformAirhouse,
        Action::PlatformOps,
        Action::PlatformApps,
        Action::PlatformExplorer,
        Action::PlatformAudit,
        Action::PlatformOrgs,
        Action::PlatformOrgCreate,
        Action::PlatformUsers,
        Action::PlatformPartners,
        Action::PlatformOperate,
        Action::PlatformGrants,
        Action::PlatformOwnerOnly,
    ];

    /// Stable log id. This is what appears in the `authz` tracing output, so treat it
    /// as a wire contract: renaming a variant is free, renaming its id breaks whatever
    /// is grepping the logs.
    fn as_str(self) -> &'static str {
        match self {
            Action::OrgRead => "org_read",
            Action::ManageLocations => "manage_locations",
            Action::ManageOrgRoles => "manage_org_roles",
            Action::ManageDocuments => "manage_documents",
            Action::ManageAssignments => "manage_assignments",
            Action::MemberInvite => "member_invite",
            Action::MemberSetRole => "member_set_role",
            Action::MemberRemove => "member_remove",
            Action::OrgBilling => "org_billing",
            Action::OrgOwnerManage => "org_owner_manage",
            Action::OrgReadStrict => "org_read_strict",
            Action::WorkspaceManage => "workspace_manage",
            Action::WorkspaceEdit => "workspace_edit",
            Action::AppAccess => "app_access",
            Action::WorkspaceDataAccess => "workspace_data_access",
            Action::AppAdmin => "app_admin",
            Action::AppAccessManage => "app_access_manage",
            Action::WorkspaceRename => "workspace_rename",
            Action::NamespaceDelete => "namespace_delete",
            Action::WorkspaceOxyAccess => "workspace_oxy_access",
            // Ids match PartnerCapability::as_str, prefixed — the partner tier's own
            // policy uses the bare cap name; these live in the shared action space.
            Action::PartnerManageMembers => "partner_manage_members",
            Action::PartnerManageApps => "partner_manage_apps",
            Action::PartnerDevelopApps => "partner_develop_apps",
            Action::PartnerViewAudit => "partner_view_audit",
            Action::PartnerManageBilling => "partner_manage_billing",
            Action::PartnerManageSecrets => "partner_manage_secrets",
            Action::PartnerCreateOrgs => "partner_create_orgs",
            Action::PartnerManageOrgSettings => "partner_manage_org_settings",
            Action::PartnerManageOltp => "partner_manage_oltp",
            Action::PlatformOltp => "platform_oltp",
            Action::PlatformAirhouse => "platform_airhouse",
            Action::PlatformOps => "platform_ops",
            Action::PlatformApps => "platform_apps",
            Action::PlatformExplorer => "platform_explorer",
            Action::PlatformAudit => "platform_audit",
            Action::PlatformOrgs => "platform_orgs",
            Action::PlatformOrgCreate => "platform_org_create",
            Action::PlatformUsers => "platform_users",
            Action::PlatformPartners => "platform_partners",
            Action::PlatformOperate => "platform_operate",
            Action::PlatformGrants => "platform_grants",
            Action::PlatformOwnerOnly => "platform_owner_only",
        }
    }

    /// Which authority ring the action requires.
    fn ring(self) -> Ring {
        match self {
            Action::OrgRead => Ring::Read,
            Action::MemberInvite | Action::MemberSetRole | Action::MemberRemove => Ring::OrgAdmin,
            Action::ManageLocations
            | Action::ManageOrgRoles
            | Action::ManageDocuments
            | Action::ManageAssignments => Ring::OrgAdmin,
            Action::OrgBilling => Ring::OrgAdminStrict,
            Action::OrgOwnerManage => Ring::OwnerOnly,
            Action::OrgReadStrict => Ring::MemberStrict,
            Action::WorkspaceManage => Ring::WorkspaceAdmin,
            Action::WorkspaceEdit => Ring::WorkspaceEdit,
            Action::AppAccess => Ring::AppAccess,
            Action::WorkspaceDataAccess => Ring::WorkspaceData,
            Action::AppAdmin => Ring::AppAdmin,
            Action::AppAccessManage => Ring::AppGrant,
            Action::WorkspaceRename | Action::NamespaceDelete => Ring::OrgAdminOrCreator,
            Action::WorkspaceOxyAccess => Ring::WorkspaceAdminStrict,
            Action::PartnerManageMembers => Ring::PartnerCap(Cap::ManageMembers),
            Action::PartnerManageApps => Ring::PartnerCap(Cap::ManageApps),
            Action::PartnerDevelopApps => Ring::PartnerCap(Cap::DevelopApps),
            Action::PartnerViewAudit => Ring::PartnerCap(Cap::ViewAudit),
            Action::PartnerManageBilling => Ring::PartnerCap(Cap::ManageBilling),
            Action::PartnerManageSecrets => Ring::PartnerCap(Cap::ManageSecrets),
            Action::PartnerCreateOrgs => Ring::PartnerCap(Cap::CreateOrgs),
            Action::PartnerManageOrgSettings => Ring::PartnerCap(Cap::ManageOrgSettings),
            Action::PartnerManageOltp => Ring::PartnerCap(Cap::ManageOrgSettings),
            // One arm for the three that share this ring: operating the
            // platform, and provisioning either data plane on a tenant's
            // behalf. Both sides of the merge added `PlatformAirhouse` to a
            // different arm — same ring, so behaviour was identical and only
            // the unreachable-pattern warning said so.
            Action::PlatformOltp | Action::PlatformAirhouse | Action::PlatformOperate => {
                Ring::PlatformCap(Cap::OperatePlatform)
            }
            Action::PlatformOps => Ring::PlatformAny,
            Action::PlatformApps => Ring::PlatformCap(Cap::ManageApps),
            Action::PlatformExplorer => Ring::PlatformCap(Cap::ViewTenants),
            Action::PlatformAudit => Ring::PlatformCap(Cap::ViewAudit),
            Action::PlatformOrgs => Ring::PlatformCap(Cap::ManageOrgSettings),
            Action::PlatformOrgCreate => Ring::PlatformCap(Cap::CreateOrgs),
            Action::PlatformUsers => Ring::PlatformCap(Cap::ManageMembers),
            Action::PlatformPartners => Ring::PlatformCap(Cap::ManagePartners),
            Action::PlatformGrants => Ring::PlatformCap(Cap::ManagePlatformGrants),
            Action::PlatformOwnerOnly => Ring::GlobalOwnerOnly,
        }
    }
}

/// A capability — the atom of authority in this model. **One vocabulary, two tiers.**
///
/// A capability names a *kind of power*, never who holds it. Both operator tiers grant
/// subsets of this same set:
///
/// * the **partner** ceiling is the first eight, one-to-one with `PartnerCapability`
///   (`cap_of` maps into them);
/// * the **platform** ceiling ([`PlatformStanding`]) may grant any of them, including
///   the three that have no partner analogue — a distributor never operates Oxy itself,
///   manages the partner registry, or reads across every tenant.
///
/// That asymmetry is the point, not an accident: `PartnerCapability` stays exhaustive at
/// eight, so adding a platform-only capability here cannot silently widen a partner.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Cap {
    ManageMembers,
    ManageApps,
    DevelopApps,
    ViewAudit,
    ManageBilling,
    ManageSecrets,
    CreateOrgs,
    ManageOrgSettings,

    // ── Platform-only (no `PartnerCapability` analogue) ───────────────────────
    /// Read-only reach into a tenant that the principal is not a member of — the
    /// support engineer's capability. Gates the staff half of [`Ring::Read`] and the
    /// admin explorer. Deliberately separate from [`Cap::ViewAudit`], which is the
    /// audit log specifically: seeing a tenant's threads is not reading its audit
    /// trail, and a role may want either without the other.
    ViewTenants,
    /// Administer the partner registry — create partners, edit their ceilings and
    /// client assignments. Platform-only by construction: a partner that could grant
    /// itself capabilities would make the ceiling meaningless.
    ManagePartners,
    /// Operate Oxy's own machinery: the worker fleet console, compile history, serve
    /// routing, platform metrics, workspace health. Infrastructure, not tenant data.
    OperatePlatform,
    /// Administer the **platform-grant table itself** — issue, re-role, re-scope and
    /// revoke other people's staff standing.
    ///
    /// This capability was owner-only for one release, on the reasoning that "a
    /// capability that could edit the grant table would let its holder widen their own
    /// grant, and the ceiling would mean nothing". That objection is real, and it is
    /// answered by [`may_delegate`] rather than by withholding the capability: a write
    /// is admissible only against a grant **strictly weaker** than the writer's own, so
    /// the one row a holder can never touch is their own. Holding this is therefore the
    /// authority to delegate *downward*, which is not the authority to escalate.
    ///
    /// Gating the door on it and nothing else would still be a hole — the guard sees a
    /// verb, not the target row. Both halves are required; see [`Action::PlatformGrants`].
    ManagePlatformGrants,
}

impl Cap {
    /// Every capability — the full platform ceiling. The partner ceiling is the first
    /// eight; see `PartnerCapability::ALL`, which stays at eight on purpose.
    pub const ALL: [Cap; 12] = [
        Cap::ManageMembers,
        Cap::ManageApps,
        Cap::DevelopApps,
        Cap::ViewAudit,
        Cap::ManageBilling,
        Cap::ManageSecrets,
        Cap::CreateOrgs,
        Cap::ManageOrgSettings,
        Cap::ViewTenants,
        Cap::ManagePartners,
        Cap::OperatePlatform,
        Cap::ManagePlatformGrants,
    ];

    /// Stable id, and a wire contract: it lands in the `authz` tracing output and is
    /// serialized outward on `/user` and the admin grant API, which the frontend gates
    /// its nav on. Renaming a variant is free; renaming an id silently empties a
    /// console.
    ///
    /// **Not** persisted — the grant table stores `role` and `scope_all`, and
    /// capabilities are derived from the role by [`PlatformRole::caps`]. That is what
    /// keeps a role's meaning editable in code rather than by `UPDATE`.
    pub fn as_str(self) -> &'static str {
        match self {
            Cap::ManageMembers => "manage_members",
            Cap::ManageApps => "manage_apps",
            Cap::DevelopApps => "develop_apps",
            Cap::ViewAudit => "view_audit",
            Cap::ManageBilling => "manage_billing",
            Cap::ManageSecrets => "manage_secrets",
            Cap::CreateOrgs => "create_orgs",
            Cap::ManageOrgSettings => "manage_org_settings",
            Cap::ViewTenants => "view_tenants",
            Cap::ManagePartners => "manage_partners",
            Cap::OperatePlatform => "operate_platform",
            Cap::ManagePlatformGrants => "manage_platform_grants",
        }
    }

    /// Parse an id back. The inverse of [`Self::as_str`], kept as a pair so the
    /// round-trip is testable and so a caller receiving a capability id over the wire
    /// (an SDK, a future policy import) can resolve it without re-deriving the mapping.
    ///
    /// Deliberately **not** load-bearing today: capabilities are derived from
    /// [`PlatformRole::caps`], never read back from storage, so nothing in production
    /// calls this. If that changes, note the fail-closed rule the loader already applies
    /// to roles — an unrecognised id must drop the grant, not be guessed at.
    pub fn from_str(s: &str) -> Option<Cap> {
        Cap::ALL.into_iter().find(|c| c.as_str() == s)
    }
}

/// Where a grant reaches. The second axis of every operator standing: capabilities say
/// **what**, scope says **where**.
///
/// `Scope::All` is not a wildcard org set — it is the absence of a boundary, which is
/// why it can't be spelled as `Orgs(every_org)`: an unbounded grant must keep covering
/// orgs created after it was issued.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Scope {
    /// Every org, present and future.
    #[default]
    All,
    /// Exactly these orgs. An empty vector reaches nothing — fail closed.
    Orgs(Vec<Uuid>),
}

impl Scope {
    pub fn covers(&self, org_id: Uuid) -> bool {
        match self {
            Scope::All => true,
            Scope::Orgs(orgs) => orgs.contains(&org_id),
        }
    }

    pub fn is_all(&self) -> bool {
        matches!(self, Scope::All)
    }

    /// Does this scope wholly contain `other`? The subset test [`may_delegate`] uses to
    /// stop a bounded operator issuing a grant that reaches further than their own.
    ///
    /// `All ⊇ everything`, including `All`. A bounded scope can never contain `All` —
    /// that asymmetry is the entire point, and writing it as `covers` over a list would
    /// silently return `true` for `Orgs([]) ⊇ Orgs([])`, which is harmless, versus
    /// `Orgs([a]) ⊇ All`, which is the escalation.
    pub fn contains(&self, other: &Scope) -> bool {
        match (self, other) {
            (Scope::All, _) => true,
            (Scope::Orgs(_), Scope::All) => false,
            (Scope::Orgs(mine), Scope::Orgs(theirs)) => theirs.iter().all(|o| mine.contains(o)),
        }
    }
}

/// A named preset over (capabilities × scope) — what a human calls "a role".
///
/// The preset is the unit that is **stored and administered**; the capability list is
/// the unit the model **decides** with. Keeping the expansion here rather than in the
/// database is what preserves the crate's policy-as-code stance: a role's meaning
/// changes by editing this file and shipping it, never by an UPDATE.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PlatformRole {
    /// Oxy ops. Everything except the owner-exclusive surfaces (the Billing queue and
    /// the grant table itself), which are gated by [`Ring::GlobalOwnerOnly`] and are
    /// therefore not expressible as a capability at all.
    GlobalAdmin,
    /// Ships and develops custom apps, and nothing else. Holds no capability that
    /// reaches org membership, org settings, billing, the partner registry, or Oxy's
    /// own infrastructure — so every tenant ring that isn't about apps evaluates to
    /// `false` for them, including org deletion.
    AppOperator,
}

impl PlatformRole {
    pub const ALL: [PlatformRole; 2] = [PlatformRole::GlobalAdmin, PlatformRole::AppOperator];

    /// The capabilities the role expands to.
    pub fn caps(self) -> Vec<Cap> {
        match self {
            // Everything a capability can express. `ManageBilling` is deliberately
            // absent: platform billing is owner-only and rides
            // [`Ring::GlobalOwnerOnly`], so granting the cap here would imply a reach
            // no ring honours — a lie in the model.
            PlatformRole::GlobalAdmin => Cap::ALL
                .into_iter()
                .filter(|c| *c != Cap::ManageBilling)
                .collect(),
            // The whole role, and the whole point: two capabilities.
            PlatformRole::AppOperator => vec![Cap::ManageApps, Cap::DevelopApps],
        }
    }

    /// Where the role sits in the staff hierarchy. Higher out-ranks lower.
    ///
    /// Exists **only** for [`may_delegate`] — no decision in `allows()` reads it, because
    /// authority comes from capabilities, not from a rank. Comparing ranks to decide
    /// access would reintroduce exactly the "one boolean, nine rings" collapse this model
    /// replaced. Delegation is the one question that is genuinely about relative
    /// standing: may *I* create *you*.
    ///
    /// The Global Owner is deliberately absent. Owner is not a row in this table and not
    /// a preset — it is the env allow-list, so it out-ranks every value here by
    /// construction and [`may_delegate`] short-circuits on it before any rank is read.
    pub fn rank(self) -> u8 {
        match self {
            PlatformRole::AppOperator => 1,
            PlatformRole::GlobalAdmin => 2,
        }
    }

    /// Stable id. Persisted in the platform-grant table — a wire contract.
    pub fn as_str(self) -> &'static str {
        match self {
            PlatformRole::GlobalAdmin => "global_admin",
            PlatformRole::AppOperator => "app_operator",
        }
    }

    /// Parse a stored id. `None` for an unknown role — the loader drops the grant
    /// rather than guessing, so an unrecognised row denies instead of escalating.
    pub fn from_str(s: &str) -> Option<PlatformRole> {
        PlatformRole::ALL.into_iter().find(|r| r.as_str() == s)
    }
}

/// One platform (Oxy-staff) standing: the capability ceiling and where it reaches.
///
/// The deliberate twin of [`PartnerStanding`] minus the `partner_id` — a platform grant
/// is not held *through* anything, which is the only structural difference between the
/// two operator tiers. Both are `(scope, caps)`; both are ceilings; neither can be
/// widened by standing held elsewhere.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlatformStanding {
    /// The role this standing was granted as, kept for display and audit. The
    /// decision reads [`Self::caps`] — never this — so a future custom cap set stays
    /// expressible without the model learning about roles.
    pub role: PlatformRole,
    pub caps: Vec<Cap>,
    pub scope: Scope,
}

impl PlatformStanding {
    /// A standing from a preset.
    pub fn from_role(role: PlatformRole, scope: Scope) -> Self {
        Self {
            role,
            caps: role.caps(),
            scope,
        }
    }

    /// Holds `cap` **anywhere** — scope not consulted. This is the console-door
    /// question ("may you open this staff surface at all"), which has no org to check
    /// against: [`Resource::platform`] is parented to nothing.
    ///
    /// A scoped operator therefore PASSES the door. Narrowing what they see behind it
    /// is a row filter the handler owns — see [`Self::scope`]. Getting this backwards
    /// gives you either a role that 403s on its own console or one that lists every
    /// tenant's apps.
    pub fn holds(&self, cap: Cap) -> bool {
        self.caps.contains(&cap)
    }

    /// Holds `cap` **over `org_id`** — capability AND scope. This is the tenant-reach
    /// question, and it is the only one that may cross an org boundary.
    pub fn grants(&self, cap: Cap, org_id: Uuid) -> bool {
        self.holds(cap) && self.scope.covers(org_id)
    }
}

/// Why a delegation was refused. Carried out to the API so the console can say which
/// bound was hit — "you cannot issue a grant wider than your own" is actionable,
/// "forbidden" sends the operator to ask someone why.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DelegationDenial {
    /// No platform standing at all — not staff.
    NotStaff,
    /// Staff, but this standing does not include [`Cap::ManagePlatformGrants`].
    NoCapability,
    /// The target's role is at or above the actor's own. **This is the arm that refuses
    /// self-edits**, and it does so structurally rather than by comparing identities:
    /// your own row necessarily carries your own role, which is never strictly below it.
    RoleNotBelow,
    /// The target reaches an org the actor's own grant does not.
    ScopeNotContained,
}

impl DelegationDenial {
    /// A stable id for the wire, and the string the console maps to its copy.
    pub fn as_str(self) -> &'static str {
        match self {
            DelegationDenial::NotStaff => "not_staff",
            DelegationDenial::NoCapability => "no_capability",
            DelegationDenial::RoleNotBelow => "role_not_below",
            DelegationDenial::ScopeNotContained => "scope_not_contained",
        }
    }
}

/// **May this principal write a grant of `(target_role × target_scope)`?**
///
/// The delegation bound, and the reason [`Cap::ManagePlatformGrants`] is safe to hand to
/// a Global Admin at all. One rule, stated once:
///
/// > A writable grant must be **strictly weaker** than the writer's own — lower role,
/// > and a scope the writer's own scope wholly contains.
///
/// Three properties fall out of that single sentence rather than needing their own
/// checks, which is why it is written as one:
///
/// * **No self-edit.** Your row carries your role, and a role is not strictly below
///   itself. You cannot re-scope or re-role yourself, so the ceiling holds.
/// * **No peer minting.** A Global Admin cannot create or delete another Global Admin.
///   Who is staff *at the top tier* stays the Global Owner's decision, and a Global
///   Admin cannot manufacture a colleague — or a sockpuppet — to act through.
/// * **No lateral widening.** An operator bounded to Acme can issue App Operators over
///   Acme and nothing else, so a bounded grant cannot launder itself into an unbounded
///   one via a second account.
///
/// Applies identically to create, re-role, re-scope and revoke. **Revoke reads the row
/// being deleted, not the caller's intent** — otherwise a Global Admin deletes a peer's
/// grant, which is both a privilege play and a denial of service on an equal.
///
/// The Global Owner short-circuits: root holds no row, so there is no rank to compare,
/// and a rank lookup for them would find nothing and refuse.
pub fn may_delegate(
    actor: &PrincipalFacts,
    target_role: PlatformRole,
    target_scope: &Scope,
) -> Result<(), DelegationDenial> {
    if actor.is_global_owner {
        return Ok(());
    }
    let standing = actor.platform.as_ref().ok_or(DelegationDenial::NotStaff)?;
    if !standing.holds(Cap::ManagePlatformGrants) {
        return Err(DelegationDenial::NoCapability);
    }
    if target_role.rank() >= standing.role.rank() {
        return Err(DelegationDenial::RoleNotBelow);
    }
    if !standing.scope.contains(target_scope) {
        return Err(DelegationDenial::ScopeNotContained);
    }
    Ok(())
}

impl PrincipalFacts {
    /// Any partner the principal operates that manages `org_id` — the coarse "operates
    /// this client", used where the shipped check is the override path rather than a
    /// specific capability.
    fn manages(&self, org_id: Uuid) -> bool {
        self.partners
            .iter()
            .any(|p| p.client_orgs.contains(&org_id))
    }

    /// A partner the principal operates that manages `org_id` AND holds `cap`. Not
    /// scoped to an acting partner: used where the shipped check resolves the partner
    /// FROM the org (the custom-app data plane), not from the URL.
    fn any_partner_grants(&self, cap: Cap, org_id: Uuid) -> bool {
        self.partners.iter().any(|p| p.grants(cap, org_id))
    }

    /// **Staff reach into a tenant**, gated on the capability that names the ring's
    /// authority and on the grant's scope. The deliberate mirror of
    /// [`Self::any_partner_grants`] — one primitive, both operator tiers.
    ///
    /// This replaces the bare `is_global_admin || is_global_owner` term that used to
    /// appear in nine tenant rings. That term is why a Global Admin could delete any
    /// org: [`Ring::OwnerOnly`] honoured it unconditionally. Now the same ring asks
    /// for [`Cap::ManageOrgSettings`], which an App Operator does not hold.
    ///
    /// The Global **Owner** short-circuit is intentional and is the one place standing
    /// is still a boolean: the owner is Oxy's root, and modelling root as a grant it
    /// could edit buys nothing.
    fn platform_grants(&self, cap: Cap, org_id: Uuid) -> bool {
        self.is_global_owner
            || self
                .platform
                .as_ref()
                .is_some_and(|p| p.grants(cap, org_id))
    }

    /// Holds `cap` with scope ignored — the platform-console door. See
    /// [`PlatformStanding::holds`] for why scope must not be consulted here.
    fn platform_holds(&self, cap: Cap) -> bool {
        self.is_global_owner || self.platform.as_ref().is_some_and(|p| p.holds(cap))
    }

    /// Any platform standing at all — a Global Owner, or a grant of any shape.
    /// Gates [`Action::PlatformOps`], the "is this person staff" question the outer
    /// `/admin/*` nest asks before any section-specific capability is consulted.
    pub fn is_staff(&self) -> bool {
        self.is_global_owner || self.platform.is_some()
    }

    /// Back-compatible read for display and telemetry: staff who are not the owner.
    /// **Not an authorization primitive** — nothing in [`allows`] reads it, and no
    /// call site should branch on it. Ask for a capability instead.
    pub fn is_global_admin(&self) -> bool {
        self.platform.is_some()
    }

    /// Where this principal's platform grant reaches, for handlers that must filter
    /// rows. `None` = no platform standing; `Some(Scope::All)` = unbounded.
    pub fn platform_scope(&self) -> Option<&Scope> {
        /// The owner's scope is unbounded and has no grant row to borrow from.
        static UNBOUNDED: Scope = Scope::All;
        if self.is_global_owner {
            return Some(&UNBOUNDED);
        }
        self.platform.as_ref().map(|p| &p.scope)
    }
}

/// The authority rings that gate an action (self is handled separately, on
/// `resource.owner`). The difference between [`Ring::OrgAdmin`] and
/// [`Ring::OrgAdminStrict`] is the global-operator override: member management honors
/// it, billing does not. The two workspace rings ride the org→workspace hierarchy (a
/// workspace is a child of its org), so an org role governs its workspaces for free;
/// [`Ring::WorkspaceAdmin`] adds the per-workspace elevation override.
///
/// Deliberately private: a ring is how the model is *stated*, not a vocabulary callers
/// choose from. Callers name an [`Action`] — the thing they are actually doing — and the
/// mapping to a ring is this crate's to decide. Making it public would let a call site
/// pick its own authority level, which is the scatter this crate exists to end.
// `PartialEq` + `Debug` are for the test that asserts every ring is reachable
// from `ALL`: an action list that omits a whole ring leaves that ring untested
// by every sweep, which is the failure this file exists to prevent. Neither
// derive widens the API — `Ring` stays private.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Ring {
    /// Any member of the resource's org, or a global admin/owner.
    Read,
    /// A **real** member of the org — no global override (the `OrgMemberStrict` gate,
    /// so a tenant's member-only reads don't leak to a cross-tenant operator).
    MemberStrict,
    /// A real org owner/admin, a managing partner, OR a global admin/owner reaching
    /// in via the cross-tenant override.
    OrgAdmin,
    /// A **real** org owner/admin only — the global-operator override and partners
    /// are excluded (the `OrgAdminStrict` billing gate).
    OrgAdminStrict,
    /// A real org **owner**, or a global operator via the synthetic-Owner override
    /// (which resolves to Owner). A partner assumes only Admin, so it's excluded
    /// (the `OrgOwner` gate).
    OwnerOnly,
    /// Effective workspace Owner/Admin: org owner/admin (via the hierarchy), a
    /// per-workspace override that elevates a member, a managing partner, or a global
    /// operator (the `WorkspaceAdmin` guard).
    WorkspaceAdmin,
    /// Effective workspace Member or above: any member of the workspace's org (every
    /// org member resolves to ≥ Member; overrides only elevate), a managing partner,
    /// or a global operator (the `WorkspaceEditor` guard).
    WorkspaceEdit,
    /// A custom app, by id (`user_can_access_app`): a real member of the app's org, a
    /// global admin, or a partner with `develop_apps` over the org. NOT the coarse
    /// managed-partner path, and NOT global owner (the check uses app-admin).
    ///
    /// Conditional on `resource.app_restricted`: a restricted app drops the plain
    /// org-membership term and demands a grant (org officers + staff +
    /// develop_apps partner remain). The grant is ANDed with org membership, so a
    /// grant is a filter on the org, never a way into it. A frontline worker enters
    /// only through a grant, restricted or not.
    AppAccess,
    /// A workspace's custom-app data plane (`check_custom_app_gates`): the
    /// unrestricted [`Self::AppAccess`] reach — member, develop_apps staff or
    /// partner — plus a frontline worker holding a grant on an app published from
    /// this workspace. Nothing here can be restricted: the resource is a workspace,
    /// and visibility is a property of an app.
    WorkspaceData,
    /// A custom app's own privileged surface: any org officer (owner/admin), an
    /// `app_members` admin row, or Oxy staff. No partner term — see
    /// [`Action::AppAdmin`].
    AppAdmin,
    /// Who may decide an app's audience: an org officer (owner/admin), Oxy staff, or
    /// a `manage_apps` partner. Distinct from [`Self::AppAdmin`] — that ring includes
    /// app admins and excludes partners; this one is the exact inverse, because
    /// running an app and staffing it are different authorities.
    AppGrant,
    /// Org owner/admin (via the hierarchy) OR the thing's creator — the plain member
    /// who made it may manage it. The creator claim reads `resource.owner`
    /// (`created_by`), and is confined to the actions that opt into this ring, so it
    /// can never leak into a privileged one.
    OrgAdminOrCreator,
    /// A **real** workspace owner/admin — the global-operator override is rejected, so
    /// no Oxy operator (and no partner, which assumes via the same override) can flip
    /// it. The workspace analogue of [`OrgAdminStrict`]; gates the Oxy-access switch,
    /// which only a real org officer may change.
    WorkspaceAdminStrict,
    /// A partner ceiling capability over the resource's org — `client ∈ partner.clients
    /// && cap ∈ partner.caps`, where the partner is the one being **acted as**, not any
    /// partner the principal happens to hold standing through.
    ///
    /// No global or membership term, deliberately: the ceiling is the whole story, so
    /// standing elsewhere cannot widen it.
    PartnerCap(Cap),
    /// Platform tier — holds ANY staff standing. The `/admin/*` door only
    /// (`oxy_owner_or_app_admin_guard`); every section behind it escalates to
    /// [`Self::PlatformCap`].
    PlatformAny,
    /// Platform tier — holds `cap`. **Scope is not consulted**: the platform resource
    /// is parented to no org, so a scoped grant passes and the handler filters rows.
    /// The twin of [`Self::PartnerCap`] without the acting-partner scoping, because a
    /// platform grant is not held through anything.
    PlatformCap(Cap),
    /// Platform tier — a global **owner** only (Billing queue, global-admin mgmt).
    GlobalOwnerOnly,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ResourceKind {
    Org,
    Workspace,
    App,
    Thread,
    /// A git namespace, a child of its org.
    Namespace,
    /// A partner org itself — the target of a capability-only console check (one with
    /// no client org yet, e.g. "may this partner create orgs").
    Partner,
    /// Oxy itself — the singleton target of the platform tier. Not org-scoped, and
    /// deliberately parented to nothing, so no org set can ever reach it.
    Platform,
}

/// The resource an action targets. Every resource belongs to an org (an `Org`
/// resource is its own scope); `owner` is set for self-owned resources (threads),
/// enabling the self ring.
#[derive(Clone, Debug)]
pub struct Resource {
    pub kind: ResourceKind,
    pub id: Uuid,
    /// The org the resource belongs to. For an `Org` resource this equals `id`.
    pub org_id: Uuid,
    pub owner: Option<Uuid>,
    /// The partner being ACTED AS, for partner-console decisions. The console is scoped
    /// to one partner by its URL, and that scope is part of the decision: holding a
    /// capability through partner B must not authorize anything while scoped to A.
    /// `None` for every non-partner decision.
    pub partner: Option<Uuid>,
    /// For an `App` resource: the app is restricted to explicitly-listed members
    /// (`apps.visibility = 'members'`). `false` — the default everywhere else —
    /// preserves the historical "any org member" rule, so this can only ever
    /// tighten a specific app and never loosens one.
    pub app_restricted: bool,
}

impl Resource {
    /// An org itself (member management, org settings).
    pub fn org(org_id: Uuid) -> Self {
        Self {
            kind: ResourceKind::Org,
            id: org_id,
            org_id,
            owner: None,
            partner: None,
            app_restricted: false,
        }
    }

    /// A workspace, scoped to the org it belongs to (so the org→workspace hierarchy
    /// resolves). Pass the org that owns the workspace as `org_id`.
    pub fn workspace(id: Uuid, org_id: Uuid) -> Self {
        Self {
            kind: ResourceKind::Workspace,
            id,
            org_id,
            owner: None,
            partner: None,
            app_restricted: false,
        }
    }

    /// Oxy itself — the singleton the platform tier targets (the admin console, the
    /// Billing queue, global-admin management). It has no org: `org_id` is nil and no
    /// platform ring reads an org set, so nothing tenant-scoped can reach it and it
    /// can't reach anything tenant-scoped.
    pub fn platform() -> Self {
        Self {
            kind: ResourceKind::Platform,
            id: Uuid::nil(),
            org_id: Uuid::nil(),
            owner: None,
            partner: None,
            app_restricted: false,
        }
    }

    /// A partner console decision with no client org — "does the partner I am acting as
    /// hold this capability at all" (e.g. create-orgs, view-audit).
    pub fn partner(partner_id: Uuid) -> Self {
        Self {
            kind: ResourceKind::Partner,
            id: partner_id,
            org_id: Uuid::nil(),
            owner: None,
            partner: Some(partner_id),
            app_restricted: false,
        }
    }

    /// A client org, reached while ACTING AS `acting_partner`. Both halves matter: the
    /// capability must come from that partner, and the org must be its client.
    pub fn partner_client(client_org_id: Uuid, acting_partner: Uuid) -> Self {
        Self {
            kind: ResourceKind::Org,
            id: client_org_id,
            org_id: client_org_id,
            owner: None,
            partner: Some(acting_partner),
            app_restricted: false,
        }
    }

    /// A workspace carrying its creator (`created_by`) as `owner`, for the one ring
    /// that honours a creator claim ([`Action::WorkspaceRename`]). Every other
    /// workspace ring ignores `owner`, so this can't widen them.
    pub fn workspace_with_creator(id: Uuid, org_id: Uuid, created_by: Option<Uuid>) -> Self {
        Self {
            kind: ResourceKind::Workspace,
            id,
            org_id,
            owner: created_by,
            partner: None,
            app_restricted: false,
        }
    }

    /// A git namespace carrying its creator as `owner`, scoped to its org — for the
    /// org-admin-or-creator ring.
    pub fn namespace_with_creator(id: Uuid, org_id: Uuid, created_by: Option<Uuid>) -> Self {
        Self {
            kind: ResourceKind::Namespace,
            id,
            org_id,
            owner: created_by,
            partner: None,
            app_restricted: false,
        }
    }

    /// A custom app, scoped to its owning org (the app is a child of the org, so
    /// `resource in <org set>` resolves). `id` may be the app/project id.
    ///
    /// Treated as **unrestricted** (`visibility = 'org'`) — the historical rule. Use
    /// [`Resource::app_with_visibility`] where the app row is in hand, so a
    /// restricted app is actually enforced.
    pub fn app(id: Uuid, org_id: Uuid) -> Self {
        Self {
            kind: ResourceKind::App,
            id,
            org_id,
            owner: None,
            partner: None,
            app_restricted: false,
        }
    }

    /// A custom app carrying its visibility. `id` MUST be the **app id** (not the
    /// project/workspace id): per-app membership is keyed by it, so passing the
    /// wrong id would silently deny every member of a restricted app.
    pub fn app_with_visibility(id: Uuid, org_id: Uuid, restricted: bool) -> Self {
        Self {
            app_restricted: restricted,
            ..Self::app(id, org_id)
        }
    }
}

/// A principal's authorization-relevant facts, loaded once per request by the app-side
/// loader. This is the crate's whole input surface: the loader states what is TRUE of
/// the principal, [`allows`] decides what that entitles them to. Keeping those two
/// apart is what lets the model be tested without a database.
///
/// Empty = a plain authenticated user with no org role and no global flag → denied
/// everything (fail closed).
#[derive(Clone, Debug, Default)]
pub struct PrincipalFacts {
    pub user_id: Uuid,
    /// Orgs where the user is **owner**.
    pub owned_orgs: Vec<Uuid>,
    /// Orgs where the user is owner **or** admin.
    pub admin_orgs: Vec<Uuid>,
    /// Every org the user is a member of (any role).
    pub member_orgs: Vec<Uuid>,
    /// Every partner this principal operates, each with its clients and its ceiling.
    ///
    /// This is deliberately NOT flattened into per-capability org sets. Flattening
    /// loses WHICH partner granted a capability, and the partner console is scoped to
    /// one acting partner (`/partners/{id}/...`) — so a flattened model answers "yes"
    /// for partner B's client while you are scoped to A, which is broader than the
    /// shipped check. Keeping the partner is what lets [`allows`] honour that scope.
    /// (The flat shape was a concession to the policy engine's `Set<Org>` attributes;
    /// without it, the model can just be correct.)
    pub partners: Vec<PartnerStanding>,
    /// Workspaces where a per-workspace `workspace_members` override raises the user
    /// to Admin or Owner above their org-derived role. Only the *exceptional* rows —
    /// the org-derived workspace role comes free from the org sets via the hierarchy,
    /// and overrides can only elevate, never downgrade.
    pub ws_admin_override: Vec<Uuid>,
    /// Apps where the principal holds an `app_members` row of ANY role. Read only
    /// by [`Ring::AppAccess`], and only when the app is restricted — so an app
    /// whose visibility is `org` behaves exactly as it did before this existed.
    pub app_memberships: Vec<Uuid>,
    /// Orgs where the principal is an **active frontline worker** — a row in
    /// `org_frontline_members`, which is deliberately NOT an `org_members` row.
    ///
    /// This is the narrowest standing in the model and it must stay that way.
    /// A frontline worker is enrolled by PIN on a shared device; giving them
    /// org membership would hand them Airhouse settings and, through
    /// `EffectiveWorkspaceRole`, Databases and Secrets. So this set appears in
    /// exactly TWO rings — [`Ring::AppAccess`], ANDed with an `app_members` grant
    /// on the app, and [`Ring::WorkspaceData`], ANDed with
    /// [`Self::frontline_workspace_grants`] — and never as a substitute for
    /// `member_orgs`.
    ///
    /// Empty for every principal who signed in with an email address.
    /// See `internal-docs/frontline-identity.md`.
    pub frontline_orgs: Vec<Uuid>,
    /// Workspaces from which an app the principal holds an `app_members` row on was
    /// published. Read by exactly one ring, [`Ring::WorkspaceData`], and only for a
    /// principal in [`Self::frontline_orgs`]: the data plane is keyed by workspace
    /// and a worker's grant by app, and this is the join between them, derived once
    /// by the loader rather than re-decided at the gate.
    ///
    /// Empty for everyone who is not a frontline worker — a member reaches the data
    /// plane through `member_orgs` and never needs it.
    pub frontline_workspace_grants: Vec<Uuid>,
    /// Apps where the principal's `app_members` row is `role = 'admin'`. A subset
    /// of [`Self::app_memberships`]; gates [`Ring::AppAdmin`].
    pub app_admin_memberships: Vec<Uuid>,
    /// This principal's Oxy-staff standing, if any: a capability ceiling and the orgs
    /// it reaches. `None` = not staff.
    ///
    /// This replaced a bare `is_global_admin: bool`. The boolean was the defect: it
    /// made every staff member identical, so the nine tenant rings that honour an
    /// operator override could not tell an app publisher from someone entitled to
    /// delete the org. Read it through [`PrincipalFacts::platform_grants`], never
    /// directly.
    pub platform: Option<PlatformStanding>,
    /// Oxy's root. Deliberately still a boolean — see
    /// [`PrincipalFacts::platform_grants`].
    pub is_global_owner: bool,
}

/// One partner the principal operates: its clients, and the ceiling over them.
#[derive(Clone, Debug)]
pub struct PartnerStanding {
    pub partner_id: Uuid,
    /// The client orgs this partner manages. Every operator reaches all of them.
    pub client_orgs: Vec<Uuid>,
    /// What Oxy granted this partner. The operator's authority IS the ceiling.
    pub caps: Vec<Cap>,
}

impl PartnerStanding {
    fn grants(&self, cap: Cap, org_id: Uuid) -> bool {
        self.caps.contains(&cap) && self.client_orgs.contains(&org_id)
    }
}

/// A denied authorization decision. The transport layer decides its HTTP shape (403
/// forbidden, or 404 to hide existence); this crate stays transport-agnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Denied {
    pub action: Action,
}

/// The decision: pure set arithmetic over the facts. **This is the whole model** — every
/// authorization decision in Oxy resolves to one arm of the `match` below, so a rule that
/// is not stated here is not a rule.
///
/// This ran on a policy engine once; the crate header records why it earned nothing.
/// What is left is the same rules in the language the rest of the codebase is written in,
/// with exhaustiveness as the compiler's job: a new [`Action`] with no `Ring`, or a
/// `Ring` with no arm here, fails the build.
pub fn allows(facts: &PrincipalFacts, action: Action, resource: &Resource) -> bool {
    // Self: a thread's owner may READ it. Scoped to BOTH kind and action — unscoped,
    // this would hand the owner of any future owner-bearing resource every action.
    if action == Action::OrgRead
        && resource.kind == ResourceKind::Thread
        && resource.owner == Some(facts.user_id)
    {
        return true;
    }

    // A resource is reached through its org: `Resource::org` sets `org_id == id` and
    // every child carries its parent's, so one containment serves both. `Platform` has
    // a nil org and is therefore in no set — nothing tenant-scoped reaches it.
    let in_org = |set: &[Uuid]| set.contains(&resource.org_id);
    // The per-workspace elevation is keyed by the workspace, not its org.
    let elevated_here =
        resource.kind == ResourceKind::Workspace && facts.ws_admin_override.contains(&resource.id);
    let is_platform = resource.kind == ResourceKind::Platform;

    // Operator reach — staff, and a managing partner — models the SYNTHETIC-OWNER
    // override, which the middleware applies only when the caller is NOT a real member.
    // Unconditional, it out-ranks a real membership and silently promotes an operator
    // who happens to be a plain member of the tenant.
    let not_member = !in_org(&facts.member_orgs);

    // Staff reach into a tenant, NAMED BY CAPABILITY. This used to be one boolean
    // (`is_global_admin || is_global_owner`) shared by every ring below, which is why
    // an app publisher could delete an org: `Ring::OwnerOnly` honoured the same term
    // `Ring::AppAdmin` did. Each ring now asks for the capability its own authority is
    // about, so a grant that omits the capability cannot reach the ring at all.
    let staff = |cap: Cap| not_member && facts.platform_grants(cap, resource.org_id);
    let staff_or_partner = |cap: Cap| {
        not_member
            && (facts.platform_grants(cap, resource.org_id) || facts.manages(resource.org_id))
    };

    match action.ring() {
        // Reading a tenant you don't belong to is the support engineer's power, and it
        // is now a capability an app-only role simply doesn't hold.
        Ring::Read => in_org(&facts.member_orgs) || staff(Cap::ViewTenants),
        Ring::MemberStrict => in_org(&facts.member_orgs),
        Ring::OrgAdmin => in_org(&facts.admin_orgs) || staff_or_partner(Cap::ManageMembers),
        // Billing: real owner/admin only — the override is barred and partners don't bill.
        Ring::OrgAdminStrict => in_org(&facts.admin_orgs),
        // Deleting an org, transferring ownership, promoting an owner. A partner assumes
        // Admin, never Owner, so no partner term here.
        //
        // THE headline of the capability split: this arm's staff term used to be the
        // bare global flag, so every Global Admin — including the ones who only ship
        // custom apps — could delete any tenant. It now demands
        // `ManageOrgSettings`, which `PlatformRole::AppOperator` does not grant.
        Ring::OwnerOnly => in_org(&facts.owned_orgs) || staff(Cap::ManageOrgSettings),
        Ring::OrgAdminOrCreator => {
            in_org(&facts.admin_orgs)
                || resource.owner == Some(facts.user_id)
                || staff_or_partner(Cap::ManageOrgSettings)
        }
        Ring::WorkspaceAdmin => {
            in_org(&facts.admin_orgs) || elevated_here || staff_or_partner(Cap::ManageOrgSettings)
        }
        // The Oxy-access switch: a REAL workspace officer; the override is rejected so
        // staff cannot unlock themselves.
        Ring::WorkspaceAdminStrict => in_org(&facts.admin_orgs) || elevated_here,
        Ring::WorkspaceEdit => {
            in_org(&facts.member_orgs) || staff_or_partner(Cap::ManageOrgSettings)
        }
        // A member of the app's org, any Oxy operator, or a develop_apps partner.
        //
        // The operator term is deliberately NOT conditioned on non-membership the way
        // the org rings are: this gate really does grant staff unconditionally, and it
        // is the same answer either way — a staff member of the org already passes on
        // the membership term.
        //
        // The partner term resolves FROM the org, not from a console URL, so it is not
        // scoped to an acting partner. And it is develop_apps, never the coarse managed
        // path: reading an app's data is not managing its lifecycle.
        Ring::AppAccess => {
            // Break-glass regardless of visibility: staff, a develop_apps partner,
            // and any org OFFICER (owner or admin — `admin_orgs` contains owners).
            // An org's own officers are never locked out of its apps.
            // `platform_grants` is called WITHOUT the `not_member` precondition the org
            // rings apply — preserved from the original: this gate really does grant
            // staff unconditionally, and it is the same answer either way, since a
            // staff member of the org already passes on the membership term.
            let unconditional = facts.platform_grants(Cap::DevelopApps, resource.org_id)
                || facts.any_partner_grants(Cap::DevelopApps, resource.org_id)
                || in_org(&facts.admin_orgs);
            // A frontline worker reaches an app ONLY through an explicit grant,
            // and only in an org they are actively enrolled in.
            //
            // Note this ignores `app_restricted` entirely: an org-visible app is
            // visible to org MEMBERS, and a frontline worker is deliberately not
            // one. Letting the unrestricted branch fall through to them would
            // hand every worker on a store's roster every app in the tenant the
            // day somebody flips one app to org-wide — which is the opposite of
            // what "frontline" is supposed to mean here.
            let frontline_grant = facts.frontline_orgs.contains(&resource.org_id)
                && facts.app_memberships.contains(&resource.id);
            if frontline_grant {
                return true;
            }
            if resource.app_restricted {
                // Restricted: plain org membership is NOT enough — that is the
                // whole point. A grant (a direct `app_members` row or one reached
                // through an `org_teams` team, either role) is.
                //
                // The grant is ANDed with org membership so it can only ever NARROW
                // the org, never widen it. Without that term a bare grant row let a
                // non-member through this ring while the DATA-plane gate
                // (`check_custom_app_gates`, which requires org membership) still
                // refused — the app's shell would load and every query would 403.
                // Grantees are validated as org members at write time too; this is
                // the enforcement half of the same rule.
                unconditional
                    || (in_org(&facts.member_orgs) && facts.app_memberships.contains(&resource.id))
            } else {
                // Unrestricted (the default): unchanged — any member of the org.
                unconditional || in_org(&facts.member_orgs)
            }
        }
        // The workspace-keyed data plane. Same reach as an UNRESTRICTED app —
        // there is no app id on the wire, so nothing here can be restricted —
        // plus the one principal `AppAccess` can only see per app: a frontline
        // worker, admitted to a workspace's data because they hold a grant on an
        // app published from it.
        //
        // The frontline term is ANDed with standing, as everywhere: a grant row
        // outliving a suspension is not access. And the grant fact is keyed by
        // the WORKSPACE the app was published from, not by the org — a worker
        // granted one store's app must not reach every workspace in the tenant.
        Ring::WorkspaceData => {
            facts.platform_grants(Cap::DevelopApps, resource.org_id)
                || facts.any_partner_grants(Cap::DevelopApps, resource.org_id)
                || in_org(&facts.member_orgs)
                || (in_org(&facts.frontline_orgs)
                    && facts.frontline_workspace_grants.contains(&resource.id))
        }
        // An org officer (owner or admin) administers every app in the org. The
        // `app_members` admin role extends that DOWNWARD to a non-officer — the
        // app's designated admin who isn't org staff (e.g. the warehouse admin) —
        // without granting org-wide billing/member powers. No develop_apps term:
        // building an app is not administering its live privileged surface.
        Ring::AppAdmin => {
            facts.platform_grants(Cap::ManageApps, resource.org_id)
                || in_org(&facts.admin_orgs)
                || facts.app_admin_memberships.contains(&resource.id)
        }
        // Staffing an app — visibility, grants, and the org team roster that feeds
        // them. An org officer, Oxy staff, or a `manage_apps` partner.
        //
        // `manage_apps` and NOT `develop_apps`: naming an audience is lifecycle, the
        // same class as publish/unpublish. The two capabilities stay split in the
        // direction that matters — this ring lets a partner decide who sees an app
        // WITHOUT letting it read the app's data, which `Ring::AppAccess` still gates
        // on `develop_apps` separately.
        //
        // No `app_admin_memberships` term: an app admin runs the app's privileged
        // surface, but deciding who reaches the app is the org's call, not the app's.
        // Otherwise an app admin could grant themselves a second app admin and the
        // org would have no way to see it coming.
        Ring::AppGrant => {
            facts.platform_grants(Cap::ManageApps, resource.org_id)
                || in_org(&facts.admin_orgs)
                || facts.any_partner_grants(Cap::ManageApps, resource.org_id)
        }
        // The console IS scoped: the capability must come from the partner being acted
        // as. Operating a partner that grants `cap` over some other client authorizes
        // nothing here — that scope is what a flattened model silently dropped.
        Ring::PartnerCap(cap) => match resource.partner {
            None => false, // a partner decision must name the partner it is acting as
            Some(acting) => facts.partners.iter().any(|p| {
                p.partner_id == acting
                    && p.caps.contains(&cap)
                    && (resource.kind == ResourceKind::Partner
                        || p.client_orgs.contains(&resource.org_id))
            }),
        },
        // The platform tier reads ONLY platform standing and is pinned to the Platform
        // singleton: no tenant standing reaches an operator surface, and no platform
        // action can be asked of a tenant resource.
        //
        // Neither arm consults SCOPE — `Resource::platform()` has a nil org, so there
        // is nothing to check it against. A scoped operator therefore passes the door
        // and the handler narrows the rows (`PrincipalFacts::platform_scope`). That
        // split is deliberate and is the one thing to get right when adding a surface:
        // capabilities gate verbs, scope filters rows.
        Ring::PlatformAny => is_platform && facts.is_staff(),
        Ring::PlatformCap(cap) => is_platform && facts.platform_holds(cap),
        Ring::GlobalOwnerOnly => is_platform && facts.is_global_owner,
    }
}

/// Enforce: `Ok(())` on allow, [`Denied`] on deny. The **end state** — the model is the
/// sole authority, with no legacy term beside it. Reach for this only where a call site
/// has no shipped check to difference against (a new surface), or where its legacy check
/// has been retired. Everywhere else, [`enforce`] keeps the old verdict as a fail-safe.
/// The caller maps [`Denied`] to its HTTP status (403 by default).
pub fn require(facts: &PrincipalFacts, action: Action, resource: &Resource) -> Result<(), Denied> {
    if allows(facts, action, resource) {
        Ok(())
    } else {
        Err(Denied { action })
    }
}

/// **The normal entry point.** The decision is `existing_allow && allows(..)`. The model
/// is binding — a deny here is a real 403 — but it sits beside the check it replaced
/// rather than on top of it.
///
/// The conjunction is deliberate and is the whole safety property: the model can only
/// ever REMOVE access the existing check already granted, so a mis-modeled ring **cannot
/// open a hole** (no cross-tenant grant is reachable through here). The residual failure
/// mode is a wrong DENY, which is loud (an immediate 403), attributable (a WARN naming
/// the label), and revertible in one line.
///
/// `existing_allow` is not ceremony — it is the **oracle**. It is the shipped check the
/// differential tests difference the model against, which is what caught every real bug
/// in this model. Passing a hand-waved `true` here silently converts a fail-safe into a
/// bare `allows` and throws that away.
///
/// This is a migration step. When a ring's legacy term is retired, call [`require`]
/// instead and delete the old branch.
pub fn enforce(
    label: &str,
    facts: &PrincipalFacts,
    action: Action,
    resource: &Resource,
    existing_allow: bool,
) -> bool {
    let unified_allow = allows(facts, action, resource);
    if unified_allow != existing_allow {
        // Enforcing, so this is not a curiosity: where the legacy check allowed and
        // the model denies, a real request just took a 403 attributable to this label.
        tracing::warn!(
            target: "authz",
            label,
            action = action.as_str(),
            user = %facts.user_id,
            org = %resource.org_id,
            unified_allow,
            existing_allow,
            newly_denied = existing_allow && !unified_allow,
            "authz disagreement — the unified model diverged from the legacy check; \
             the conjunction denies. Investigate the ring."
        );
    }
    // Can only subtract. A hole is unreachable; a wrong deny is visible above.
    existing_allow && unified_allow
}

/// **Authoritative mode.** The model DECIDES; the legacy verdict is kept only as an
/// OBSERVER. The inverse of [`enforce`]: the model's verdict is returned, and
/// `legacy_observed` is compared to it purely to keep the disagreement signal alive.
///
/// This drops the fail-safe conjunction, so a mis-modeled ring CAN open a hole — the one
/// failure [`enforce`] makes unreachable. Use it only where differential tests prove the
/// ring matches its oracle across the scenario space, and prefer it over [`require`] only
/// when you still want the legacy signal on real traffic.
///
/// **Currently unwired**, deliberately: no call site has passed a seeded-database
/// differential run yet, and dropping a fail-safe on the strength of hand-built facts
/// would be trusting an assumption about the loader rather than the loader.
///
/// When the observer is no longer wanted, call [`require`] — the legacy branch is then
/// dead and can be deleted.
pub fn authorize(
    label: &str,
    facts: &PrincipalFacts,
    action: Action,
    resource: &Resource,
    legacy_observed: bool,
) -> bool {
    let unified_allow = allows(facts, action, resource);
    if unified_allow != legacy_observed {
        tracing::warn!(
            target: "authz",
            label,
            action = action.as_str(),
            user = %facts.user_id,
            org = %resource.org_id,
            unified_allow,
            legacy_observed,
            authority = "unified",
            "authz disagreement — the unified model decided; the legacy check would have \
             differed. The model's answer stands; investigate the ring."
        );
    }
    unified_allow
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    fn org() -> Uuid {
        Uuid::from_u128(1)
    }
    fn other_org() -> Uuid {
        Uuid::from_u128(2)
    }
    fn user() -> Uuid {
        Uuid::from_u128(100)
    }

    /// A partner standing: `p(client, &[caps])` — operated partner id defaults to
    /// `partner_org()` unless given.
    fn standing(partner_id: Uuid, client: Uuid, caps: &[Cap]) -> PartnerStanding {
        PartnerStanding {
            partner_id,
            client_orgs: vec![client],
            caps: caps.to_vec(),
        }
    }
    fn partner_org() -> Uuid {
        Uuid::from_u128(900)
    }

    fn facts() -> PrincipalFacts {
        PrincipalFacts {
            user_id: user(),
            ..Default::default()
        }
    }

    /// An unscoped Global Admin — the standing that replaced `is_global_admin: true`.
    /// Every assertion written against the old boolean holds unchanged against this,
    /// which is the property that makes the capability split non-breaking.
    fn global_admin_standing() -> Option<PlatformStanding> {
        Some(PlatformStanding::from_role(
            PlatformRole::GlobalAdmin,
            Scope::All,
        ))
    }

    /// An App Operator: `{ManageApps, DevelopApps}` and nothing else.
    fn app_operator_standing(scope: Scope) -> Option<PlatformStanding> {
        Some(PlatformStanding::from_role(
            PlatformRole::AppOperator,
            scope,
        ))
    }

    /// Airhouse provisioning rides `OperatePlatform`, so it must answer exactly
    /// as the action it shares that ring with — a divergence would mean a grant
    /// that operates the platform but cannot provision a warehouse, which
    /// nothing in the product describes.
    #[test]
    fn airhouse_provisioning_answers_as_platform_operate_does() {
        for f in [
            PrincipalFacts {
                platform: global_admin_standing(),
                ..facts()
            },
            PrincipalFacts {
                platform: app_operator_standing(Scope::All),
                ..facts()
            },
            PrincipalFacts {
                owned_orgs: vec![org()],
                ..facts()
            },
            facts(),
        ] {
            assert_eq!(
                allows(&f, Action::PlatformAirhouse, &Resource::platform()),
                allows(&f, Action::PlatformOperate, &Resource::platform()),
                "airhouse must not diverge from the ring it rides"
            );
        }
    }

    #[test]
    fn org_admin_may_manage_members() {
        let f = PrincipalFacts {
            admin_orgs: vec![org()],
            member_orgs: vec![org()],
            ..facts()
        };
        assert!(allows(&f, Action::MemberSetRole, &Resource::org(org())));
        assert!(allows(&f, Action::OrgRead, &Resource::org(org())));
    }

    #[test]
    fn global_flags_must_not_grant_over_a_real_membership() {
        // An Oxy staffer who is ALSO a plain member of a customer org. The legacy
        // guard denies: org_context hands them their REAL row (role = Member,
        // is_global_override = false), so `matches!(role, Owner|Admin)` is false.
        //
        // The global flags model the SYNTHETIC-OWNER override path — which only
        // applies when the operator is NOT a real member. As unconditional facts they
        // would fire here too and hand a plain member org-admin: a privilege
        // escalation, live the moment the legacy term is dropped.
        let staffer_who_is_a_plain_member = PrincipalFacts {
            member_orgs: vec![org()],
            platform: global_admin_standing(),
            ..facts()
        };
        assert!(!allows(
            &staffer_who_is_a_plain_member,
            Action::MemberSetRole,
            &Resource::org(org())
        ));
        // Same for a global owner.
        let owner_who_is_a_plain_member = PrincipalFacts {
            member_orgs: vec![org()],
            is_global_owner: true,
            ..facts()
        };
        assert!(!allows(
            &owner_who_is_a_plain_member,
            Action::MemberSetRole,
            &Resource::org(org())
        ));
        // The override path itself must still work: NOT a real member -> the operator
        // reaches in (org_context synthesizes Owner, and the legacy check agrees).
        let staffer_not_a_member = PrincipalFacts {
            platform: global_admin_standing(),
            ..facts()
        };
        assert!(allows(
            &staffer_not_a_member,
            Action::MemberSetRole,
            &Resource::org(org())
        ));
    }

    #[test]
    fn plain_member_reads_but_cannot_manage() {
        let f = PrincipalFacts {
            member_orgs: vec![org()],
            ..facts()
        };
        assert!(allows(&f, Action::OrgRead, &Resource::org(org())));
        assert!(!allows(&f, Action::MemberSetRole, &Resource::org(org())));
        assert!(!allows(&f, Action::OrgBilling, &Resource::org(org())));
    }

    #[test]
    fn billing_admits_real_owner_and_admin_but_not_override_or_partner() {
        // OrgAdminStrict: a REAL owner or admin bills; the cross-tenant global
        // override and partners do not.
        let real_owner = PrincipalFacts {
            owned_orgs: vec![org()],
            admin_orgs: vec![org()],
            member_orgs: vec![org()],
            ..facts()
        };
        let real_admin = PrincipalFacts {
            admin_orgs: vec![org()],
            member_orgs: vec![org()],
            ..facts()
        };
        // A global operator who is NOT a real member reaches the org only via the
        // synthetic-Owner override, which billing rejects.
        let global_only = PrincipalFacts {
            platform: global_admin_standing(),
            is_global_owner: true,
            ..facts()
        };
        let partner = PrincipalFacts {
            partners: vec![standing(partner_org(), org(), &Cap::ALL)],
            ..facts()
        };
        assert!(allows(
            &real_owner,
            Action::OrgBilling,
            &Resource::org(org())
        ));
        assert!(allows(
            &real_admin,
            Action::OrgBilling,
            &Resource::org(org())
        ));
        assert!(!allows(
            &global_only,
            Action::OrgBilling,
            &Resource::org(org())
        ));
        assert!(!allows(&partner, Action::OrgBilling, &Resource::org(org())));
    }

    #[test]
    fn partner_may_manage_a_managed_org() {
        let f = PrincipalFacts {
            partners: vec![standing(partner_org(), org(), &Cap::ALL)],
            ..facts()
        };
        assert!(allows(&f, Action::MemberSetRole, &Resource::org(org())));
        // ...but not billing (partner ceiling doesn't reach OrgAdminStrict).
        assert!(!allows(&f, Action::OrgBilling, &Resource::org(org())));
    }

    #[test]
    fn global_reaches_member_mgmt_via_override_but_not_billing() {
        let ga = PrincipalFacts {
            platform: global_admin_standing(),
            ..facts()
        };
        let go = PrincipalFacts {
            is_global_owner: true,
            ..facts()
        };
        // Member management honors the cross-tenant override — both globals pass.
        assert!(allows(&ga, Action::MemberSetRole, &Resource::org(org())));
        assert!(allows(&go, Action::MemberSetRole, &Resource::org(org())));
        // Billing (OrgAdminStrict) rejects the override — neither global reaches it.
        assert!(!allows(&ga, Action::OrgBilling, &Resource::org(org())));
        assert!(!allows(&go, Action::OrgBilling, &Resource::org(org())));
    }

    #[test]
    fn wrong_org_and_outsider_are_denied() {
        // Admin of a DIFFERENT org.
        let elsewhere = PrincipalFacts {
            admin_orgs: vec![other_org()],
            member_orgs: vec![other_org()],
            ..facts()
        };
        assert!(!allows(
            &elsewhere,
            Action::MemberSetRole,
            &Resource::org(org())
        ));
        assert!(!allows(&elsewhere, Action::OrgRead, &Resource::org(org())));
        // A plain authenticated user with no facts.
        assert!(!allows(&facts(), Action::OrgRead, &Resource::org(org())));
    }

    #[test]
    fn self_owned_resource_is_reachable_by_its_owner() {
        let f = facts();
        let thread = Resource {
            kind: ResourceKind::Thread,
            id: Uuid::from_u128(500),
            org_id: org(),
            owner: Some(user()),
            partner: None,
            app_restricted: false,
        };
        assert!(allows(&f, Action::OrgRead, &thread));
        // A different user does not own it.
        let other = PrincipalFacts {
            user_id: Uuid::from_u128(101),
            ..Default::default()
        };
        assert!(!allows(&other, Action::OrgRead, &thread));
    }

    #[test]
    fn self_rule_grants_only_reads_and_only_to_owner_bearing_threads() {
        // Pins the scope of the self rule, so a future owner-bearing resource (or a
        // privileged action on an owned thread) can never ride it into an allow once
        // rings flip to enforce.
        let f = facts();
        let thread = Resource {
            kind: ResourceKind::Thread,
            id: Uuid::from_u128(500),
            org_id: org(),
            owner: Some(user()),
            partner: None,
            app_restricted: false,
        };
        // Owning a thread grants the read...
        assert!(allows(&f, Action::OrgRead, &thread));
        // ...and NEVER a privileged action, even on the resource you own.
        assert!(!allows(&f, Action::OrgBilling, &thread));
        assert!(!allows(&f, Action::MemberSetRole, &thread));
        assert!(!allows(&f, Action::WorkspaceManage, &thread));
        assert!(!allows(&f, Action::OrgOwnerManage, &thread));

        // An owner-bearing resource of a DIFFERENT kind gets no self grant at all —
        // this is the case that would otherwise become a hole.
        let owned_workspace = Resource {
            kind: ResourceKind::Workspace,
            id: Uuid::from_u128(701),
            org_id: org(),
            owner: Some(user()),
            partner: None,
            app_restricted: false,
        };
        assert!(!allows(&f, Action::OrgRead, &owned_workspace));
        assert!(!allows(&f, Action::WorkspaceManage, &owned_workspace));
        assert!(!allows(&f, Action::WorkspaceEdit, &owned_workspace));
    }

    #[test]
    fn workspace_manage_needs_admin_or_per_workspace_override() {
        let ws = Uuid::from_u128(700);
        let r = Resource::workspace(ws, org());
        // Org owner/admin govern the workspace through the org hierarchy.
        let org_admin = PrincipalFacts {
            admin_orgs: vec![org()],
            member_orgs: vec![org()],
            ..facts()
        };
        assert!(allows(&org_admin, Action::WorkspaceManage, &r));
        // A plain org member does NOT manage it...
        let member = PrincipalFacts {
            member_orgs: vec![org()],
            ..facts()
        };
        assert!(!allows(&member, Action::WorkspaceManage, &r));
        // ...unless a per-workspace override elevates them on THIS workspace.
        let elevated = PrincipalFacts {
            member_orgs: vec![org()],
            ws_admin_override: vec![ws],
            ..facts()
        };
        assert!(allows(&elevated, Action::WorkspaceManage, &r));
        // An override on a different workspace doesn't help.
        let elsewhere = PrincipalFacts {
            member_orgs: vec![org()],
            ws_admin_override: vec![Uuid::from_u128(701)],
            ..facts()
        };
        assert!(!allows(&elsewhere, Action::WorkspaceManage, &r));
        // A managing partner (assumes Admin) reaches it.
        let partner = PrincipalFacts {
            partners: vec![standing(partner_org(), org(), &Cap::ALL)],
            ..facts()
        };
        assert!(allows(&partner, Action::WorkspaceManage, &r));
    }

    #[test]
    fn workspace_edit_is_any_member_partner_or_global() {
        let ws = Uuid::from_u128(700);
        let r = Resource::workspace(ws, org());
        let member = PrincipalFacts {
            member_orgs: vec![org()],
            ..facts()
        };
        let partner = PrincipalFacts {
            partners: vec![standing(partner_org(), org(), &Cap::ALL)],
            ..facts()
        };
        let global = PrincipalFacts {
            platform: global_admin_standing(),
            ..facts()
        };
        let outsider = PrincipalFacts {
            member_orgs: vec![other_org()],
            ..facts()
        };
        assert!(allows(&member, Action::WorkspaceEdit, &r));
        assert!(allows(&partner, Action::WorkspaceEdit, &r));
        assert!(allows(&global, Action::WorkspaceEdit, &r));
        assert!(!allows(&outsider, Action::WorkspaceEdit, &r));
    }

    #[test]
    fn owner_only_is_real_owner_or_global_not_admin_or_partner() {
        let r = Resource::org(org());
        let owner = PrincipalFacts {
            owned_orgs: vec![org()],
            admin_orgs: vec![org()],
            member_orgs: vec![org()],
            ..facts()
        };
        // A real admin is NOT an owner.
        let admin = PrincipalFacts {
            admin_orgs: vec![org()],
            member_orgs: vec![org()],
            ..facts()
        };
        // A partner assumes Admin, never Owner.
        let partner = PrincipalFacts {
            partners: vec![standing(partner_org(), org(), &Cap::ALL)],
            ..facts()
        };
        // A global operator reaches in as the synthetic Owner.
        let global = PrincipalFacts {
            platform: global_admin_standing(),
            ..facts()
        };
        assert!(allows(&owner, Action::OrgOwnerManage, &r));
        assert!(!allows(&admin, Action::OrgOwnerManage, &r));
        assert!(!allows(&partner, Action::OrgOwnerManage, &r));
        assert!(allows(&global, Action::OrgOwnerManage, &r));
    }

    #[test]
    fn member_strict_is_real_member_only_no_global_or_partner() {
        let r = Resource::org(org());
        let member = PrincipalFacts {
            member_orgs: vec![org()],
            ..facts()
        };
        // A global operator with no real membership must NOT get a member-strict read
        // (billing-status/checkout must not leak cross-tenant).
        let global = PrincipalFacts {
            platform: global_admin_standing(),
            is_global_owner: true,
            ..facts()
        };
        let partner = PrincipalFacts {
            partners: vec![standing(partner_org(), org(), &Cap::ALL)],
            ..facts()
        };
        assert!(allows(&member, Action::OrgReadStrict, &r));
        assert!(!allows(&global, Action::OrgReadStrict, &r));
        assert!(!allows(&partner, Action::OrgReadStrict, &r));
    }

    #[test]
    fn app_access_is_member_global_admin_or_develop_apps_partner() {
        let app = Resource::app(Uuid::from_u128(900), org());
        // A real member of the app's org.
        let member = PrincipalFacts {
            member_orgs: vec![org()],
            ..facts()
        };
        // An Oxy global admin.
        let ga = PrincipalFacts {
            platform: global_admin_standing(),
            ..facts()
        };
        // A partner whose ceiling grants develop_apps over the org.
        let dev_partner = PrincipalFacts {
            partners: vec![standing(partner_org(), org(), &[Cap::DevelopApps])],
            ..facts()
        };
        assert!(allows(&member, Action::AppAccess, &app));
        assert!(allows(&ga, Action::AppAccess, &app));
        assert!(allows(&dev_partner, Action::AppAccess, &app));
        // A managing partner WITHOUT develop_apps must NOT reach the data plane — that
        // is the manage_apps vs develop_apps split.
        let manage_only = PrincipalFacts {
            partners: vec![standing(partner_org(), org(), &[Cap::ManageApps])],
            ..facts()
        };
        assert!(!allows(&manage_only, Action::AppAccess, &app));
        // A global owner reaches it too: both operator tiers reach every custom-app
        // surface. This used to be admin-only, so an owner not in `app_admins` could
        // PUBLISH an app but not VIEW one.
        let go = PrincipalFacts {
            is_global_owner: true,
            ..facts()
        };
        assert!(allows(&go, Action::AppAccess, &app));
        // A plain outsider is denied.
        assert!(!allows(&facts(), Action::AppAccess, &app));
    }

    #[test]
    fn restricted_app_drops_plain_org_membership_but_keeps_break_glass() {
        let app_id = Uuid::from_u128(950);
        let open = Resource::app_with_visibility(app_id, org(), false);
        let restricted = Resource::app_with_visibility(app_id, org(), true);

        // A plain org member: reaches an open app, DENIED a restricted one. This
        // is the subtraction the whole feature exists for.
        let member = PrincipalFacts {
            member_orgs: vec![org()],
            ..facts()
        };
        assert!(allows(&member, Action::AppAccess, &open));
        assert!(!allows(&member, Action::AppAccess, &restricted));

        // The same member WITH an app_members row reaches it (either role).
        let app_member = PrincipalFacts {
            member_orgs: vec![org()],
            app_memberships: vec![app_id],
            ..facts()
        };
        assert!(allows(&app_member, Action::AppAccess, &restricted));

        // A membership in a DIFFERENT app doesn't help.
        let elsewhere = PrincipalFacts {
            member_orgs: vec![org()],
            app_memberships: vec![Uuid::from_u128(951)],
            ..facts()
        };
        assert!(!allows(&elsewhere, Action::AppAccess, &restricted));

        // Break-glass: an org officer (owner OR admin) and Oxy staff still reach a
        // restricted app, so an org can't lock itself (or support) out of its app.
        let owner = PrincipalFacts {
            owned_orgs: vec![org()],
            admin_orgs: vec![org()],
            member_orgs: vec![org()],
            ..facts()
        };
        let org_admin = PrincipalFacts {
            admin_orgs: vec![org()],
            member_orgs: vec![org()],
            ..facts()
        };
        let staff = PrincipalFacts {
            platform: global_admin_standing(),
            ..facts()
        };
        assert!(allows(&owner, Action::AppAccess, &restricted));
        assert!(allows(&org_admin, Action::AppAccess, &open));
        assert!(allows(&org_admin, Action::AppAccess, &restricted));
        assert!(allows(&staff, Action::AppAccess, &restricted));

        // A develop_apps partner keeps its data-plane reach.
        let dev_partner = PrincipalFacts {
            partners: vec![standing(partner_org(), org(), &[Cap::DevelopApps])],
            ..facts()
        };
        assert!(allows(&dev_partner, Action::AppAccess, &restricted));

        // An outsider is denied either way.
        let outsider = PrincipalFacts {
            member_orgs: vec![other_org()],
            ..facts()
        };
        assert!(!allows(&outsider, Action::AppAccess, &open));
        assert!(!allows(&outsider, Action::AppAccess, &restricted));
    }

    #[test]
    fn app_admin_is_an_org_officer_the_app_admin_role_or_staff() {
        let app_id = Uuid::from_u128(960);
        let app = Resource::app_with_visibility(app_id, org(), false);

        // The feature's reason to exist: an app admin who is only a plain org
        // member — admin rights extended DOWNWARD to a non-officer.
        let app_admin = PrincipalFacts {
            member_orgs: vec![org()],
            app_memberships: vec![app_id],
            app_admin_memberships: vec![app_id],
            ..facts()
        };
        assert!(allows(&app_admin, Action::AppAdmin, &app));

        // A non-admin app member reaches the app but NOT its privileged surface.
        let app_member = PrincipalFacts {
            member_orgs: vec![org()],
            app_memberships: vec![app_id],
            ..facts()
        };
        assert!(allows(&app_member, Action::AppAccess, &app));
        assert!(!allows(&app_member, Action::AppAdmin, &app));

        // Any org officer (owner OR admin) administers every app in the org, and
        // staff are break-glass.
        let owner = PrincipalFacts {
            owned_orgs: vec![org()],
            admin_orgs: vec![org()],
            member_orgs: vec![org()],
            ..facts()
        };
        let org_admin = PrincipalFacts {
            admin_orgs: vec![org()],
            member_orgs: vec![org()],
            ..facts()
        };
        let staff = PrincipalFacts {
            is_global_owner: true,
            ..facts()
        };
        assert!(allows(&owner, Action::AppAdmin, &app));
        assert!(allows(&org_admin, Action::AppAdmin, &app));
        assert!(allows(&staff, Action::AppAdmin, &app));

        // But a plain org MEMBER (no app-admin row) does not administer it — the
        // line is at officer, not member.
        let plain_member = PrincipalFacts {
            member_orgs: vec![org()],
            ..facts()
        };
        assert!(!allows(&plain_member, Action::AppAdmin, &app));

        // Admin of a DIFFERENT app grants nothing here.
        let other_app_admin = PrincipalFacts {
            member_orgs: vec![org()],
            app_admin_memberships: vec![Uuid::from_u128(961)],
            ..facts()
        };
        assert!(!allows(&other_app_admin, Action::AppAdmin, &app));

        // A develop_apps partner builds the app but does not administer it.
        let dev_partner = PrincipalFacts {
            partners: vec![standing(partner_org(), org(), &Cap::ALL)],
            ..facts()
        };
        assert!(!allows(&dev_partner, Action::AppAdmin, &app));

        // A plain member and an outsider hold nothing.
        assert!(!allows(&facts(), Action::AppAdmin, &app));
    }

    #[test]
    fn restricted_grant_is_a_filter_on_the_org_not_a_way_into_it() {
        // A grant NARROWS an org; it never widens one. Before the org-membership
        // term, a bare `app_memberships` entry passed this ring while the data-plane
        // gate (which requires org membership) still refused — the app shell loaded
        // and every query 403'd. Grantees are validated as org members at write
        // time; this is the enforcement half of that rule.
        let app_id = Uuid::from_u128(980);
        let restricted = Resource::app_with_visibility(app_id, org(), true);

        let granted_outsider = PrincipalFacts {
            app_memberships: vec![app_id],
            ..facts() // note: NO member_orgs
        };
        assert!(!allows(&granted_outsider, Action::AppAccess, &restricted));

        // The same grant, held by a real org member, does reach it.
        let granted_member = PrincipalFacts {
            member_orgs: vec![org()],
            app_memberships: vec![app_id],
            ..facts()
        };
        assert!(allows(&granted_member, Action::AppAccess, &restricted));

        // Membership of a DIFFERENT org doesn't satisfy the new term either.
        let wrong_org = PrincipalFacts {
            member_orgs: vec![Uuid::from_u128(981)],
            app_memberships: vec![app_id],
            ..facts()
        };
        assert!(!allows(&wrong_org, Action::AppAccess, &restricted));
    }

    #[test]
    fn shaping_the_org_is_the_same_authority_as_staffing_it() {
        // Locations and tenant-defined roles land on `Ring::OrgAdmin` — the
        // same ring as member management — rather than getting one of their
        // own. Both decide the SHAPE of the org rather than doing work inside
        // it, and a synonym ring is a second place for the answer to drift.
        let org_admin = PrincipalFacts {
            admin_orgs: vec![org()],
            ..facts()
        };
        for a in [
            Action::ManageLocations,
            Action::ManageOrgRoles,
            Action::ManageAssignments,
        ] {
            assert!(allows(&org_admin, a, &Resource::org(org())));
        }

        // A plain member may not. This is the load-bearing half: a store
        // manager holding a tenant-defined role is not thereby able to invent
        // new ones, or to create the locations work is routed to.
        let member = PrincipalFacts {
            member_orgs: vec![org()],
            ..facts()
        };
        for a in [
            Action::ManageLocations,
            Action::ManageOrgRoles,
            Action::ManageAssignments,
        ] {
            assert!(!allows(&member, a, &Resource::org(org())));
        }
    }

    #[test]
    fn publishing_to_the_org_is_an_admin_act_and_a_worker_may_not_do_it() {
        // `ManageDocuments` joins the two shape-changing actions above on
        // `Ring::OrgAdmin`. What is worth pinning is the LOWER edge, because
        // this action's whole subject is the frontline: the Knowledge base
        // exists to be read on a tablet in a kitchen, and the person holding
        // that tablet must not be able to edit the SOP they are following.
        let org_admin = PrincipalFacts {
            admin_orgs: vec![org()],
            ..facts()
        };
        assert!(allows(
            &org_admin,
            Action::ManageDocuments,
            &Resource::org(org())
        ));

        // A plain org member — an assistant manager with a login — may read
        // everything published and write none of it in v1.
        let member = PrincipalFacts {
            member_orgs: vec![org()],
            ..facts()
        };
        assert!(!allows(
            &member,
            Action::ManageDocuments,
            &Resource::org(org())
        ));

        // The narrowest standing there is. A frontline worker is enrolled by
        // PIN on a shared device and holds no `org_members` row, so the only
        // way this could ever pass is if some ring started reading
        // `frontline_orgs` as if it were membership. That is the failure this
        // assertion exists to catch.
        let worker = PrincipalFacts {
            frontline_orgs: vec![org()],
            ..facts()
        };
        assert!(!allows(
            &worker,
            Action::ManageDocuments,
            &Resource::org(org())
        ));

        // Admin of a DIFFERENT org. Documents are the first org-wide library
        // in the model, so "admin somewhere" reaching one is the cross-tenant
        // shape both #3050 and #3048 shipped once.
        let elsewhere = PrincipalFacts {
            admin_orgs: vec![other_org()],
            member_orgs: vec![other_org()],
            ..facts()
        };
        assert!(!allows(
            &elsewhere,
            Action::ManageDocuments,
            &Resource::org(org())
        ));
    }

    /// `ALL` is internally consistent — and what that does NOT prove.
    ///
    /// `ALL` is a hand-written array, so adding an `Action` and forgetting to
    /// list it compiles cleanly and drops that action from every sweep that
    /// iterates it, including `an_empty_principal_is_denied_every_action`
    /// below — the only fail-closed check over the whole surface. A new ring
    /// would then be untested by the test written to catch exactly that.
    ///
    /// Not hypothetical: merging main into the document branch got this array's
    /// length wrong, and the only thing that objected was its type annotation.
    ///
    /// **Completeness is not enforced here and cannot be.** Rust has no way to
    /// enumerate an enum's variants without a derive, and this crate is
    /// deliberately limited to `uuid` + `tracing` — the constraint that lets the
    /// model be tested without a database (`crates/authz/CLAUDE.md`). Adding
    /// `strum` to buy one assertion would spend that.
    ///
    /// So this checks the two halves that ARE checkable, and the practical
    /// backstop for the third is `as_str`: its match has no wildcard, so a new
    /// variant fails to compile until somebody names it, and the person naming
    /// it is looking at this array three screens up.
    #[test]
    fn all_is_internally_consistent() {
        let mut names: Vec<&str> = Action::ALL.iter().map(|a| a.as_str()).collect();
        let declared = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(
            declared,
            names.len(),
            "`ALL` lists the same action twice — a sweep would test it twice and another not at all"
        );
        // NOT a second length check — `declared` is `ALL.len()` by
        // construction, so comparing the two could never fail, and the first
        // version of this test shipped that tautology under a message implying
        // it checked the `[Action; 42]` annotation. That is the compiler's job
        // and the docstring above says so.
        //
        // Every ring must be reachable instead: an action whose ring nothing in
        // `ALL` maps to is a ring no differential case can ever exercise, which
        // is the failure this file exists to prevent.
        // Every ring an action can name must be named by SOME action in `ALL`.
        // Iterating the actions and collecting their rings would be circular;
        // this walks the mapping the other way, from a list of rings that the
        // `ring()` match must stay exhaustive over.
        for ring in [
            Ring::Read,
            Ring::MemberStrict,
            Ring::OrgAdmin,
            Ring::OrgAdminStrict,
            Ring::OwnerOnly,
            Ring::WorkspaceAdmin,
            Ring::WorkspaceEdit,
            Ring::AppAccess,
            Ring::WorkspaceData,
        ] {
            assert!(
                Action::ALL.iter().any(|a| a.ring() == ring),
                "no action in `ALL` sits on {ring:?} — that ring is unreachable from every \
                 sweep that iterates `ALL`, including the fail-closed one below"
            );
        }
    }

    #[test]
    fn an_empty_principal_is_denied_every_action() {
        // Fail-closed, restated across the two new actions as well as the
        // existing ones — a new `Action` that forgot its ring would show up
        // here as a principal with no facts being allowed something.
        //
        // What this does NOT test, and cannot: that no ring reads a
        // tenant-defined role. That is a compile-time property — there is
        // deliberately no `PrincipalFacts` field carrying one, so holding
        // "Store Manager" is unreadable by `allows` rather than merely
        // ignored. If a future change adds such a field, the argument for it
        // belongs in that change; pretending to assert it here would be a test
        // that passes for the wrong reason.
        let nobody = facts();
        for a in Action::ALL {
            assert!(
                !allows(&nobody, a, &Resource::org(org())),
                "{a:?} was allowed for a principal with no facts"
            );
        }
    }

    #[test]
    fn a_frontline_worker_reaches_exactly_the_apps_they_were_granted() {
        // The narrowest standing in the model. A frontline worker is enrolled by
        // PIN on a shared tablet and holds NO org membership by design — giving
        // them one would hand them Airhouse settings and, via
        // EffectiveWorkspaceRole, Databases and Secrets.
        let granted = Uuid::from_u128(1101);
        let other = Uuid::from_u128(1102);

        let worker = PrincipalFacts {
            frontline_orgs: vec![org()],
            app_memberships: vec![granted],
            ..facts() // note: NO member_orgs, NO admin_orgs
        };

        // The granted app, restricted or not.
        assert!(allows(
            &worker,
            Action::AppAccess,
            &Resource::app_with_visibility(granted, org(), true)
        ));
        assert!(allows(
            &worker,
            Action::AppAccess,
            &Resource::app_with_visibility(granted, org(), false)
        ));

        // An app in the same org they were NOT granted — including an org-wide
        // one. This is the case worth pinning: an org-visible app is visible to
        // org MEMBERS, and a worker is not one. If this ever flips, every worker
        // on a store's roster gets every app in the tenant the day somebody
        // makes one app org-wide.
        assert!(!allows(
            &worker,
            Action::AppAccess,
            &Resource::app_with_visibility(other, org(), false)
        ));
    }

    #[test]
    fn frontline_standing_does_not_leak_past_its_org_or_into_other_rings() {
        let app_id = Uuid::from_u128(1103);
        let elsewhere = Uuid::from_u128(1104);

        // Standing in a DIFFERENT org does not reach this org's app, even with
        // the grant row — the two terms are ANDed for exactly this reason.
        let wrong_org = PrincipalFacts {
            frontline_orgs: vec![elsewhere],
            app_memberships: vec![app_id],
            ..facts()
        };
        assert!(!allows(
            &wrong_org,
            Action::AppAccess,
            &Resource::app_with_visibility(app_id, org(), true)
        ));

        // And in the right org, the standing buys NOTHING outside the two app
        // rings (`AppAccess`, and `WorkspaceData` below). If a later change reads
        // `frontline_orgs` in a third, this is what catches it.
        let worker = PrincipalFacts {
            frontline_orgs: vec![org()],
            app_memberships: vec![app_id],
            ..facts()
        };
        assert!(!allows(&worker, Action::OrgRead, &Resource::org(org())));
        assert!(!allows(
            &worker,
            Action::AppAdmin,
            &Resource::app(app_id, org())
        ));
    }

    #[test]
    fn workspace_data_plane_admits_members_and_granted_workers_only() {
        let ws = Uuid::from_u128(1201);
        let next_door = Uuid::from_u128(1202);
        let app_here = Uuid::from_u128(1203);
        let here = Resource::workspace(ws, org());

        // A member reaches their org's data plane, and nobody reaches a foreign one.
        let member = PrincipalFacts {
            member_orgs: vec![org()],
            ..facts()
        };
        assert!(allows(&member, Action::WorkspaceDataAccess, &here));
        assert!(!allows(
            &member,
            Action::WorkspaceDataAccess,
            &Resource::workspace(ws, Uuid::from_u128(7))
        ));

        // A worker granted an app published from THIS workspace: in. The same
        // worker at the workspace next door: out. The grant is per workspace,
        // never per org — this is the case the gate used to hand-decide because
        // the ring, asked about a workspace, could not see an app grant at all.
        let worker = PrincipalFacts {
            frontline_orgs: vec![org()],
            app_memberships: vec![app_here],
            frontline_workspace_grants: vec![ws],
            ..facts() // NO member_orgs
        };
        assert!(allows(&worker, Action::WorkspaceDataAccess, &here));
        assert!(!allows(
            &worker,
            Action::WorkspaceDataAccess,
            &Resource::workspace(next_door, org())
        ));

        // Standing without a grant, and a grant without standing — the rows a
        // suspended worker leaves behind. Both out; the two facts are ANDed.
        let standing_only = PrincipalFacts {
            frontline_orgs: vec![org()],
            ..facts()
        };
        assert!(!allows(&standing_only, Action::WorkspaceDataAccess, &here));
        let rows_only = PrincipalFacts {
            app_memberships: vec![app_here],
            frontline_workspace_grants: vec![ws],
            ..facts()
        };
        assert!(!allows(&rows_only, Action::WorkspaceDataAccess, &here));

        // The workspace fact buys a worker nothing outside this ring: not the
        // workspace itself, not the org.
        assert!(!allows(&worker, Action::WorkspaceEdit, &here));
        assert!(!allows(&worker, Action::WorkspaceManage, &here));
        assert!(!allows(&worker, Action::OrgRead, &Resource::org(org())));
    }

    #[test]
    fn app_grant_ring_is_officer_staff_or_manage_apps_partner() {
        let app = Resource::app(Uuid::from_u128(990), org());

        // Org officers decide their org's audiences.
        let owner = PrincipalFacts {
            owned_orgs: vec![org()],
            admin_orgs: vec![org()],
            member_orgs: vec![org()],
            ..facts()
        };
        let org_admin = PrincipalFacts {
            admin_orgs: vec![org()],
            member_orgs: vec![org()],
            ..facts()
        };
        assert!(allows(&owner, Action::AppAccessManage, &app));
        assert!(allows(&org_admin, Action::AppAccessManage, &app));

        // Both Oxy operator tiers reach it.
        for staff in [
            PrincipalFacts {
                platform: global_admin_standing(),
                ..facts()
            },
            PrincipalFacts {
                is_global_owner: true,
                ..facts()
            },
        ] {
            assert!(allows(&staff, Action::AppAccessManage, &app));
        }

        // A `manage_apps` partner CAN — naming an audience is app lifecycle. This is
        // the exact inverse of AppAdmin, which has no partner term at all.
        let manage_partner = PrincipalFacts {
            partners: vec![standing(partner_org(), org(), &[Cap::ManageApps])],
            ..facts()
        };
        assert!(allows(&manage_partner, Action::AppAccessManage, &app));
        assert!(!allows(&manage_partner, Action::AppAdmin, &app));

        // ...but a `develop_apps`-only partner CANNOT. The split holds in this
        // direction too: building an app is not staffing it.
        let dev_only = PrincipalFacts {
            partners: vec![standing(partner_org(), org(), &[Cap::DevelopApps])],
            ..facts()
        };
        assert!(!allows(&dev_only, Action::AppAccessManage, &app));

        // A capability held over a DIFFERENT org authorizes nothing here.
        let elsewhere = PrincipalFacts {
            partners: vec![standing(
                partner_org(),
                Uuid::from_u128(991),
                &[Cap::ManageApps],
            )],
            ..facts()
        };
        assert!(!allows(&elsewhere, Action::AppAccessManage, &app));

        // A plain org member cannot restaff an app, and neither can an app admin —
        // running an app's privileged surface is not deciding who reaches it.
        let plain_member = PrincipalFacts {
            member_orgs: vec![org()],
            ..facts()
        };
        assert!(!allows(&plain_member, Action::AppAccessManage, &app));
        let app_admin = PrincipalFacts {
            member_orgs: vec![org()],
            app_memberships: vec![app.id],
            app_admin_memberships: vec![app.id],
            ..facts()
        };
        assert!(allows(&app_admin, Action::AppAdmin, &app));
        assert!(!allows(&app_admin, Action::AppAccessManage, &app));

        // An outsider holds nothing.
        assert!(!allows(&facts(), Action::AppAccessManage, &app));
    }

    #[test]
    fn app_membership_facts_do_not_leak_into_other_rings() {
        // Pins the blast radius: holding an app-admin row must not confer org or
        // workspace authority — the failure mode if a future ring reads these sets.
        let app_id = Uuid::from_u128(970);
        let f = PrincipalFacts {
            member_orgs: vec![org()],
            app_memberships: vec![app_id],
            app_admin_memberships: vec![app_id],
            ..facts()
        };
        assert!(!allows(&f, Action::MemberSetRole, &Resource::org(org())));
        assert!(!allows(&f, Action::OrgBilling, &Resource::org(org())));
        assert!(!allows(&f, Action::OrgOwnerManage, &Resource::org(org())));
        assert!(!allows(
            &f,
            Action::WorkspaceManage,
            &Resource::workspace(Uuid::from_u128(971), org())
        ));
        assert!(!allows(&f, Action::PlatformOps, &Resource::platform()));
    }

    #[test]
    fn partner_ceiling_is_per_capability_and_scoped_to_the_acting_partner() {
        let a = partner_org();
        let b = Uuid::from_u128(901);
        // Acting as partner A, on A's client.
        let as_a = Resource::partner_client(org(), a);

        // A grants manage_apps (lifecycle) but NOT develop_apps (the data plane). This
        // split is the one a coarse model collapses — and collapsing it hands a
        // lifecycle-only partner another tenant's app data.
        let lifecycle_only = PrincipalFacts {
            partners: vec![standing(a, org(), &[Cap::ManageApps])],
            ..facts()
        };
        assert!(allows(&lifecycle_only, Action::PartnerManageApps, &as_a));
        assert!(!allows(&lifecycle_only, Action::PartnerDevelopApps, &as_a));
        assert!(!allows(
            &lifecycle_only,
            Action::AppAccess,
            &Resource::org(org())
        ));
        // Holding one capability grants no other.
        assert!(!allows(
            &lifecycle_only,
            Action::PartnerManageBilling,
            &as_a
        ));
        assert!(!allows(&lifecycle_only, Action::PartnerViewAudit, &as_a));

        // THE SCOPE PROPERTY. Operating BOTH A and B, where only B grants view_audit:
        // acting as A must not borrow B's ceiling, even over a client both manage.
        let two = PrincipalFacts {
            partners: vec![
                standing(a, org(), &[Cap::ManageApps]),
                standing(b, org(), &[Cap::ViewAudit]),
            ],
            ..facts()
        };
        assert!(allows(
            &two,
            Action::PartnerViewAudit,
            &Resource::partner_client(org(), b)
        ));
        assert!(
            !allows(&two, Action::PartnerViewAudit, &as_a),
            "acting as A must not borrow B's ceiling — the scope a flattened model drops"
        );

        // A capability over one client says nothing about another.
        let elsewhere = PrincipalFacts {
            partners: vec![standing(a, org(), &[Cap::ViewAudit])],
            ..facts()
        };
        assert!(!allows(
            &elsewhere,
            Action::PartnerViewAudit,
            &Resource::partner_client(other_org(), a)
        ));

        // A partner decision must NAME the partner being acted as; an unscoped resource
        // can never authorize one.
        assert!(!allows(
            &two,
            Action::PartnerViewAudit,
            &Resource::org(org())
        ));

        // An empty ceiling holds nothing; a plain member holds no partner capability.
        let no_ceiling = PrincipalFacts {
            partners: vec![standing(a, org(), &[])],
            ..facts()
        };
        for cap in Cap::ALL {
            // The platform-only capabilities have no partner action, and that is the
            // asymmetry the two tiers are built on: a distributor can never operate
            // Oxy, edit the partner registry, or read across every tenant. `None` here
            // is an assertion, not a gap — if one of these ever gains a partner action
            // the ceiling has been widened and this arm must be reconsidered.
            let action = match cap {
                Cap::ManageMembers => Action::PartnerManageMembers,
                Cap::ManageApps => Action::PartnerManageApps,
                Cap::DevelopApps => Action::PartnerDevelopApps,
                Cap::ViewAudit => Action::PartnerViewAudit,
                Cap::ManageBilling => Action::PartnerManageBilling,
                Cap::ManageSecrets => Action::PartnerManageSecrets,
                Cap::CreateOrgs => Action::PartnerCreateOrgs,
                Cap::ManageOrgSettings => Action::PartnerManageOrgSettings,
                Cap::ViewTenants
                | Cap::ManagePartners
                | Cap::OperatePlatform
                | Cap::ManagePlatformGrants => continue,
            };
            assert!(
                !allows(&no_ceiling, action, &as_a),
                "{cap:?} leaked with no ceiling"
            );
        }
        let member = PrincipalFacts {
            member_orgs: vec![org()],
            ..facts()
        };
        assert!(!allows(&member, Action::PartnerManageApps, &as_a));
    }

    #[test]
    fn an_assumed_standing_is_the_ceiling_and_no_more() {
        // Staff looking through a partner's eyes get a standing exactly like a real
        // operator's — the partner's ceiling, never more. This is what the model used
        // to be blind to: the console's scope knew about the override and the facts
        // didn't, so the rings would have denied staff their own console.
        let a = partner_org();
        let assumed = PrincipalFacts {
            partners: vec![standing(a, org(), &[Cap::ManageApps, Cap::ViewAudit])],
            // Staff, but that must not add anything on the partner rings.
            is_global_owner: true,
            ..facts()
        };
        let as_a = Resource::partner_client(org(), a);
        assert!(allows(&assumed, Action::PartnerManageApps, &as_a));
        assert!(allows(&assumed, Action::PartnerViewAudit, &as_a));
        // Off in the ceiling stays off — on a PARTNER ring, staff see the same walls the
        // customer does. Assuming a partner does not widen what that partner may do.
        assert!(!allows(&assumed, Action::PartnerManageBilling, &as_a));
        assert!(!allows(&assumed, Action::PartnerDevelopApps, &as_a));
        // AppAccess is deliberately NOT asserted here: it is not a partner ring. It
        // grants any Oxy operator directly, so this principal reaches it on their staff
        // standing regardless of the ceiling — a different question with a different
        // answer, and conflating the two is how a ring gets modeled wrong.
    }

    #[test]
    fn workspace_rename_admits_org_admin_or_the_creator_only() {
        let ws = Uuid::from_u128(800);
        // A plain member who CREATED this workspace.
        let creator = PrincipalFacts {
            member_orgs: vec![org()],
            ..facts()
        };
        let by_creator = Resource::workspace_with_creator(ws, org(), Some(user()));
        assert!(allows(&creator, Action::WorkspaceRename, &by_creator));

        // The same plain member on a workspace someone ELSE created: denied.
        let by_other = Resource::workspace_with_creator(ws, org(), Some(Uuid::from_u128(101)));
        assert!(!allows(&creator, Action::WorkspaceRename, &by_other));
        // ...and when the creator is unknown.
        let by_nobody = Resource::workspace_with_creator(ws, org(), None);
        assert!(!allows(&creator, Action::WorkspaceRename, &by_nobody));

        // An org admin renames any workspace, creator or not.
        let admin = PrincipalFacts {
            admin_orgs: vec![org()],
            member_orgs: vec![org()],
            ..facts()
        };
        assert!(allows(&admin, Action::WorkspaceRename, &by_other));

        // The creator claim is confined to rename — it must not imply a privileged
        // workspace action.
        assert!(!allows(&creator, Action::WorkspaceManage, &by_creator));
        // ...nor reach an outsider's workspace.
        let outsider = PrincipalFacts {
            member_orgs: vec![other_org()],
            ..facts()
        };
        assert!(!allows(&outsider, Action::WorkspaceRename, &by_other));
    }

    #[test]
    fn platform_tier_reads_only_global_flags_and_is_unreachable_from_a_tenant_role() {
        let p = Resource::platform();
        let ga = PrincipalFacts {
            platform: global_admin_standing(),
            ..facts()
        };
        let go = PrincipalFacts {
            is_global_owner: true,
            ..facts()
        };
        // platform_ops: either global.
        assert!(allows(&ga, Action::PlatformOps, &p));
        assert!(allows(&go, Action::PlatformOps, &p));
        // There is no admin-only action: both tiers reach every operator surface.
        // platform_owner_only: OWNER only.
        assert!(allows(&go, Action::PlatformOwnerOnly, &p));
        assert!(!allows(&ga, Action::PlatformOwnerOnly, &p));

        // The isolation property: no tenant standing — however senior — reaches the
        // platform. Platform is parentless, so no org set can resolve onto it.
        let org_owner = PrincipalFacts {
            owned_orgs: vec![org()],
            admin_orgs: vec![org()],
            member_orgs: vec![org()],
            partners: vec![standing(partner_org(), org(), &Cap::ALL)],
            ..facts()
        };
        assert!(!allows(&org_owner, Action::PlatformOps, &p));
        assert!(!allows(&org_owner, Action::PlatformOwnerOnly, &p));
        // And the converse: a platform action can't be smuggled onto a tenant org.
        assert!(!allows(
            &go,
            Action::PlatformOwnerOnly,
            &Resource::org(org())
        ));
    }

    // ── The capability split ──────────────────────────────────────────────────
    // What the platform tier buys once staff standing is `(scope × caps)` instead of a
    // boolean. Each test below fails against the old model.

    /// Provisioning an OLTP database creates a **billable** project at the
    /// provider, so it sits behind `OperatePlatform` rather than the staff
    /// door. An App Operator ships apps and nothing else.
    #[test]
    fn oltp_provisioning_is_operator_work_not_app_work() {
        let ga = PrincipalFacts {
            platform: global_admin_standing(),
            ..facts()
        };
        assert!(allows(&ga, Action::PlatformOltp, &Resource::platform()));

        let op = PrincipalFacts {
            platform: app_operator_standing(Scope::All),
            ..facts()
        };
        assert!(
            !allows(&op, Action::PlatformOltp, &Resource::platform()),
            "an app operator must not provision a billable database"
        );

        // And no tenant standing reaches the operator surface, however senior.
        let owner = PrincipalFacts {
            owned_orgs: [org()].into(),
            ..facts()
        };
        assert!(
            !allows(&owner, Action::PlatformOltp, &Resource::platform()),
            "an org owner has no platform reach"
        );
    }

    /// Airhouse rides the same ring, and must answer identically for every
    /// standing — a divergence would mean a grant that provisions one data
    /// plane but not the other, which nothing in the product describes.
    #[test]
    fn airhouse_provisioning_answers_exactly_as_oltp_does() {
        for facts in [
            PrincipalFacts {
                platform: global_admin_standing(),
                ..facts()
            },
            PrincipalFacts {
                platform: app_operator_standing(Scope::All),
                ..facts()
            },
            PrincipalFacts {
                owned_orgs: [org()].into(),
                ..facts()
            },
            facts(),
        ] {
            assert_eq!(
                allows(&facts, Action::PlatformAirhouse, &Resource::platform()),
                allows(&facts, Action::PlatformOltp, &Resource::platform()),
                "airhouse and oltp must not diverge"
            );
        }
    }

    /// A partner provisions for the client orgs it manages, and only those.
    #[test]
    fn a_partner_provisions_oltp_only_for_its_own_clients() {
        let f = PrincipalFacts {
            partners: vec![standing(partner_org(), org(), &[Cap::ManageOrgSettings])],
            ..facts()
        };
        assert!(allows(
            &f,
            Action::PartnerManageOltp,
            &Resource::partner_client(org(), partner_org())
        ));

        let someone_elses = Uuid::from_u128(4242);
        assert!(
            !allows(
                &f,
                Action::PartnerManageOltp,
                &Resource::partner_client(someone_elses, partner_org())
            ),
            "a partner must not provision for an org it does not manage"
        );

        // The capability is the gate, not partnership itself.
        let no_cap = PrincipalFacts {
            partners: vec![standing(partner_org(), org(), &[Cap::DevelopApps])],
            ..facts()
        };
        assert!(!allows(
            &no_cap,
            Action::PartnerManageOltp,
            &Resource::partner_client(org(), partner_org())
        ));
    }

    /// **The bug this design exists to fix.** `Ring::OwnerOnly` gates
    /// `Action::OrgOwnerManage` — "delete, ownership transfer, owner-promotion" — and
    /// its staff term used to be the bare global flag. Every Global Admin could
    /// therefore delete any tenant, including the ones who only ship custom apps.
    #[test]
    fn an_app_operator_cannot_delete_an_org() {
        let op = PrincipalFacts {
            platform: app_operator_standing(Scope::All),
            ..facts()
        };
        assert!(
            !allows(&op, Action::OrgOwnerManage, &Resource::org(org())),
            "an app operator must never reach org deletion / ownership transfer"
        );

        // The Global Admin preset still does, so this subtracts from exactly one role.
        let ga = PrincipalFacts {
            platform: global_admin_standing(),
            ..facts()
        };
        assert!(allows(&ga, Action::OrgOwnerManage, &Resource::org(org())));
    }

    /// Everything else an app-only role must not inherit from staff standing.
    #[test]
    fn an_app_operator_holds_no_tenant_authority_beyond_apps() {
        let op = PrincipalFacts {
            platform: app_operator_standing(Scope::All),
            ..facts()
        };
        let o = Resource::org(org());
        let ws = Resource::workspace(Uuid::from_u128(77), org());

        for (action, resource) in [
            (Action::MemberInvite, &o),
            (Action::MemberSetRole, &o),
            (Action::MemberRemove, &o),
            (Action::OrgOwnerManage, &o),
            (Action::OrgBilling, &o),
            (Action::OrgRead, &o),
            (Action::WorkspaceManage, &ws),
            (Action::WorkspaceEdit, &ws),
            (Action::WorkspaceRename, &ws),
            (Action::WorkspaceOxyAccess, &ws),
        ] {
            assert!(
                !allows(&op, action, resource),
                "{action:?} leaked to an app operator"
            );
        }
    }

    /// The other half: the role must actually work. Both app capabilities reach their
    /// rings, on a restricted app as well as an open one.
    #[test]
    fn an_app_operator_reaches_every_app_ring() {
        let op = PrincipalFacts {
            platform: app_operator_standing(Scope::All),
            ..facts()
        };
        let app_id = Uuid::from_u128(55);
        let open = Resource::app(app_id, org());
        let restricted = Resource::app_with_visibility(app_id, org(), true);

        assert!(allows(&op, Action::AppAccess, &open));
        assert!(allows(&op, Action::AppAccess, &restricted));
        assert!(allows(&op, Action::AppAdmin, &open));
        assert!(allows(&op, Action::AppAccessManage, &open));
    }

    /// Console sections are gated by capability, so one `/admin` shell serves several
    /// staff roles. The DOOR (`PlatformOps`) opens for any standing — that is what
    /// makes it a door and not an authority.
    #[test]
    fn console_sections_are_gated_by_capability_not_by_being_staff() {
        let p = Resource::platform();
        let op = PrincipalFacts {
            platform: app_operator_standing(Scope::All),
            ..facts()
        };

        assert!(allows(&op, Action::PlatformOps, &p), "the door must open");
        assert!(allows(&op, Action::PlatformApps, &p));

        for action in [
            Action::PlatformOrgs,
            Action::PlatformOrgCreate,
            Action::PlatformUsers,
            Action::PlatformPartners,
            Action::PlatformOperate,
            Action::PlatformAudit,
            Action::PlatformExplorer,
            Action::PlatformOwnerOnly,
        ] {
            assert!(
                !allows(&op, action, &p),
                "{action:?} leaked to an app operator"
            );
        }

        // A Global Admin still reaches every section except the owner-only ones.
        let ga = PrincipalFacts {
            platform: global_admin_standing(),
            ..facts()
        };
        for action in [
            Action::PlatformApps,
            Action::PlatformOrgs,
            Action::PlatformOrgCreate,
            Action::PlatformUsers,
            Action::PlatformPartners,
            Action::PlatformOperate,
            Action::PlatformAudit,
            Action::PlatformExplorer,
        ] {
            assert!(
                allows(&ga, action, &p),
                "{action:?} regressed for a global admin"
            );
        }
        assert!(!allows(&ga, Action::PlatformOwnerOnly, &p));
    }

    /// **Caps gate verbs; scope filters rows.** A scoped operator passes the console
    /// door — there is no org on `Resource::platform()` to check scope against — and is
    /// narrowed inside its tenant reach. Handlers own the row filter
    /// (`PrincipalFacts::platform_scope`); asserting the door closes here would be
    /// asserting the wrong design.
    #[test]
    fn scope_narrows_tenant_reach_and_deliberately_not_the_console_door() {
        let mine = org();
        let theirs = Uuid::from_u128(4242);
        let op = PrincipalFacts {
            platform: app_operator_standing(Scope::Orgs(vec![mine])),
            ..facts()
        };

        assert!(allows(
            &op,
            Action::AppAdmin,
            &Resource::app(Uuid::from_u128(1), mine)
        ));
        assert!(
            !allows(
                &op,
                Action::AppAdmin,
                &Resource::app(Uuid::from_u128(2), theirs)
            ),
            "scope must fence tenant reach"
        );

        // The door, by design, does not consult scope.
        assert!(allows(&op, Action::PlatformApps, &Resource::platform()));
        assert_eq!(op.platform_scope(), Some(&Scope::Orgs(vec![mine])));
    }

    /// An empty scope reaches nothing — fail closed, not "unbounded by omission".
    #[test]
    fn an_empty_scope_grants_nothing() {
        let op = PrincipalFacts {
            platform: app_operator_standing(Scope::Orgs(vec![])),
            ..facts()
        };
        assert!(!allows(
            &op,
            Action::AppAdmin,
            &Resource::app(Uuid::from_u128(1), org())
        ));
    }

    /// The Global Owner is still a boolean and still reaches everything, scope-free.
    #[test]
    fn the_global_owner_is_unaffected_by_the_capability_split() {
        let go = PrincipalFacts {
            is_global_owner: true,
            ..facts()
        };
        assert!(allows(&go, Action::OrgOwnerManage, &Resource::org(org())));
        assert!(allows(
            &go,
            Action::PlatformOwnerOnly,
            &Resource::platform()
        ));
        assert!(allows(&go, Action::PlatformApps, &Resource::platform()));
        assert!(go.platform_scope().is_some_and(Scope::is_all));
    }

    /// Staff standing must not out-rank a REAL membership — the pre-existing rule,
    /// re-pinned now that the term is per-capability rather than one flag.
    #[test]
    fn a_capability_still_does_not_out_rank_a_real_membership() {
        let staffer_who_is_a_plain_member = PrincipalFacts {
            member_orgs: vec![org()],
            platform: global_admin_standing(),
            ..facts()
        };
        assert!(
            !allows(
                &staffer_who_is_a_plain_member,
                Action::MemberSetRole,
                &Resource::org(org())
            ),
            "a staffer who is a real plain member is a plain member there"
        );
    }

    /// Stored ids are a wire contract: every capability and role round-trips, and an
    /// id this build doesn't know is DROPPED rather than guessed — so rolling back
    /// past a capability's introduction narrows reach instead of widening it.
    #[test]
    fn capability_and_role_ids_round_trip_and_unknown_ids_are_dropped() {
        for cap in Cap::ALL {
            assert_eq!(Cap::from_str(cap.as_str()), Some(cap));
        }
        for role in PlatformRole::ALL {
            assert_eq!(PlatformRole::from_str(role.as_str()), Some(role));
        }
        assert_eq!(Cap::from_str("manage_everything"), None);
        assert_eq!(PlatformRole::from_str("superuser"), None);
    }

    /// The preset is the unit that is administered; these are the exact expansions.
    #[test]
    fn role_presets_expand_to_the_documented_capabilities() {
        assert_eq!(
            PlatformRole::AppOperator.caps(),
            vec![Cap::ManageApps, Cap::DevelopApps]
        );
        // Platform billing rides `Ring::GlobalOwnerOnly`, so granting the cap would
        // imply a reach no ring honours.
        assert!(
            !PlatformRole::GlobalAdmin
                .caps()
                .contains(&Cap::ManageBilling)
        );
        assert_eq!(PlatformRole::GlobalAdmin.caps().len(), Cap::ALL.len() - 1);
    }

    // The delegation bound. Every assertion below is written so that deleting ONE
    // guard in `may_delegate` reddens at least one of them — the file-wide
    // `contains("may_delegate")` style of check is what let three earlier defects
    // through on this branch.

    fn admin_over(scope: Scope) -> PrincipalFacts {
        PrincipalFacts {
            platform: Some(PlatformStanding::from_role(
                PlatformRole::GlobalAdmin,
                scope,
            )),
            ..facts()
        }
    }

    #[test]
    fn owner_may_delegate_anything_including_a_peer_tier_grant() {
        let go = PrincipalFacts {
            is_global_owner: true,
            ..facts()
        };
        // Root, and holding no row: the short-circuit is load-bearing, because a rank
        // lookup for the owner finds nothing.
        assert_eq!(
            may_delegate(&go, PlatformRole::GlobalAdmin, &Scope::All),
            Ok(())
        );
        assert_eq!(
            may_delegate(&go, PlatformRole::AppOperator, &Scope::All),
            Ok(())
        );
        assert_eq!(
            may_delegate(&go, PlatformRole::AppOperator, &Scope::Orgs(vec![org()])),
            Ok(())
        );
    }

    #[test]
    fn unbounded_admin_may_issue_app_operators_at_any_scope() {
        let ga = admin_over(Scope::All);
        assert_eq!(
            may_delegate(&ga, PlatformRole::AppOperator, &Scope::All),
            Ok(())
        );
        assert_eq!(
            may_delegate(&ga, PlatformRole::AppOperator, &Scope::Orgs(vec![org()])),
            Ok(())
        );
    }

    #[test]
    fn admin_may_not_mint_or_touch_a_peer() {
        let ga = admin_over(Scope::All);
        // Pins the `>=` in the rank comparison. A `>` would let a Global Admin create
        // another Global Admin, and — since delete reads the target row — delete one.
        // That is peer minting and peer removal, and it is the Owner's call.
        assert_eq!(
            may_delegate(&ga, PlatformRole::GlobalAdmin, &Scope::All),
            Err(DelegationDenial::RoleNotBelow)
        );
        assert_eq!(
            may_delegate(&ga, PlatformRole::GlobalAdmin, &Scope::Orgs(vec![org()])),
            Err(DelegationDenial::RoleNotBelow),
            "narrowing the scope must not buy a peer-tier grant"
        );
    }

    #[test]
    fn self_edit_is_refused_structurally() {
        // The actor IS a `global_admin`, so their own row's role is `global_admin`,
        // which is never strictly below itself. No identity comparison is involved
        // and none is needed — this is why the rule is one sentence and not three.
        let bounded = admin_over(Scope::Orgs(vec![org()]));
        assert_eq!(
            may_delegate(&bounded, PlatformRole::GlobalAdmin, &Scope::All),
            Err(DelegationDenial::RoleNotBelow),
            "a bounded admin widening their own row to scope_all is THE escalation \
             this bound exists to stop"
        );
    }

    #[test]
    fn bounded_admin_may_not_issue_beyond_its_own_orgs() {
        let a = org();
        let b = Uuid::from_u128(0xBEEF);
        let ga = admin_over(Scope::Orgs(vec![a]));

        assert_eq!(
            may_delegate(&ga, PlatformRole::AppOperator, &Scope::Orgs(vec![a])),
            Ok(())
        );
        // Pins `Scope::contains`'s (Orgs, All) => false arm. Without it a bounded
        // operator launders itself unbounded through a second account.
        assert_eq!(
            may_delegate(&ga, PlatformRole::AppOperator, &Scope::All),
            Err(DelegationDenial::ScopeNotContained)
        );
        // Pins the subset test rather than mere non-emptiness.
        assert_eq!(
            may_delegate(&ga, PlatformRole::AppOperator, &Scope::Orgs(vec![b])),
            Err(DelegationDenial::ScopeNotContained)
        );
        assert_eq!(
            may_delegate(&ga, PlatformRole::AppOperator, &Scope::Orgs(vec![a, b])),
            Err(DelegationDenial::ScopeNotContained),
            "a superset containing one permitted org is still a superset"
        );
    }

    #[test]
    fn app_operator_and_non_staff_may_not_delegate_at_all() {
        let op = PrincipalFacts {
            platform: Some(PlatformStanding::from_role(
                PlatformRole::AppOperator,
                Scope::All,
            )),
            ..facts()
        };
        // Capability, not rank, is what stops this — an App Operator has no
        // `ManagePlatformGrants`, so it fails before any comparison.
        assert_eq!(
            may_delegate(&op, PlatformRole::AppOperator, &Scope::All),
            Err(DelegationDenial::NoCapability)
        );
        assert_eq!(
            may_delegate(&facts(), PlatformRole::AppOperator, &Scope::All),
            Err(DelegationDenial::NotStaff)
        );
    }

    #[test]
    fn scope_contains_is_a_subset_test_not_an_overlap_test() {
        let a = org();
        let b = Uuid::from_u128(0xBEEF);
        assert!(Scope::All.contains(&Scope::All));
        assert!(Scope::All.contains(&Scope::Orgs(vec![a])));
        assert!(!Scope::Orgs(vec![a]).contains(&Scope::All));
        assert!(Scope::Orgs(vec![a, b]).contains(&Scope::Orgs(vec![a])));
        assert!(!Scope::Orgs(vec![a]).contains(&Scope::Orgs(vec![a, b])));
        // An empty target reaches nothing, so every scope contains it. Harmless, and
        // stated so a reader does not mistake it for a hole.
        assert!(Scope::Orgs(vec![]).contains(&Scope::Orgs(vec![])));
    }

    #[test]
    fn manage_platform_grants_is_an_admin_capability_only() {
        assert!(
            PlatformRole::GlobalAdmin
                .caps()
                .contains(&Cap::ManagePlatformGrants)
        );
        assert!(
            !PlatformRole::AppOperator
                .caps()
                .contains(&Cap::ManagePlatformGrants)
        );
        // The door and the fence are different questions; both must exist. Asserted
        // through `allows` rather than by comparing rings: `Ring` is private on purpose
        // (a public ring lets a call site pick its own authority level), and asserting
        // the decision is the stronger claim anyway.
        let p = Resource::platform();
        let ga = PrincipalFacts {
            platform: global_admin_standing(),
            ..facts()
        };
        let op = PrincipalFacts {
            platform: Some(PlatformStanding::from_role(
                PlatformRole::AppOperator,
                Scope::All,
            )),
            ..facts()
        };
        assert!(allows(&ga, Action::PlatformGrants, &p));
        assert!(
            !allows(&op, Action::PlatformGrants, &p),
            "an App Operator must not reach the grant console at all"
        );
        assert!(
            !allows(&facts(), Action::PlatformGrants, &p),
            "a non-staff principal must not reach the grant console"
        );
    }
}
