//! Turn a [`Caller`] into [`PrincipalFacts`] — the org sets, partner standings
//! and global flags [`oxy_authz::allows`] decides over. This is consolidation, not a new
//! authority; every fact comes from an existing primitive:
//!
//! * org roles    → one `org_members` query, with owner/admin/member derived from the
//!   rows in memory ([`derive_org_roles`])
//! * partner sets → bounded queries over active grants + partner access + managed
//!   clients, and their ceilings — straight from `partner_authz::operated_partners` /
//!   `assumed_partners`, the single source `resolve_scope` also reads. This once
//!   hand-rolled those queries under a "keep the two in sync" comment, which was the
//!   drift it exists to prevent; there is one implementation now.
//! * ws overrides → one `workspace_members` query, only the exceptional elevations
//! * global admin → [`is_app_admin_email`] (`app_admins` table)
//! * global owner → [`is_oxy_owner`] (`OXY_OWNER` env allow-list)
//!
//! Cost (design §5): the guards call this at most ONCE per request — `authz::
//! request_facts` memoizes it in the request's extensions. The partner queries
//! short-circuit to empty for the common non-partner user, so a typical request is
//! three queries: memberships + ws-overrides + app-admins.
//!
//! ## A token caps the facts here, at the source
//!
//! The facts are loaded for a [`Caller`], which carries the API token the request
//! arrived with. A token that narrows — grants instead of all-access, `platform` or
//! `partner` off — gets the same principal's facts **capped** (API-tokens design §4.4):
//! the org sets shrink to the orgs a grant reaches at each level, the workspace
//! elevations to the workspaces a grant covers at admin, platform standing to the orgs
//! the token names (or to nothing), partner standings likewise. `allows()` then applies
//! the exact, per-target cap from [`PrincipalFacts::token`]. A browser session and a
//! legacy key load exactly what they always did.

use entity::org_members::OrgRole;
use entity::prelude::*;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use oxy_authz::{Cap, PartnerStanding, PrincipalFacts, RoleCeiling, ServiceAccountStanding};

use crate::caller::Caller;
use crate::globals;
use crate::partner_authz;

/// Load the authorization-relevant facts for a principal. `None` means **"we don't
/// know"** — a lookup errored — and is deliberately not the same value as `Some(facts)`
/// with empty sets, which means "we asked, and this principal belongs to nothing".
///
/// ## Why this returns an Option
///
/// This used to collapse a failed query to an empty set and call that "fail closed,
/// so the worst case is more denial". That reasoning is right in isolation and wrong
/// here. Under `enforce`, empty sets are read as *facts*: the model concludes the
/// principal is in no org, denies, and `existing_allow && false` turns a transient blip
/// — a statement timeout, a serialization failure, a dropped socket — into a **403 for
/// a legitimate org officer on every guarded route** until it clears.
///
/// It also quietly broke the promise `server::authz` makes: "a database blip must not
/// lock every tenant out of their org". That fail-safe only ever covered a failure to
/// *establish* a connection (`request_facts` → `None`); a connection that is up but
/// errors on one lookup sailed straight past it and denied.
///
/// So the failure is surfaced instead of averaged into the data, and each caller says
/// what unknown means for it. There is no safe blanket answer: an `enforce` caller can
/// defer to `existing_allow` (the conjunction only ever subtracts, so deferring cannot
/// open a hole), but a caller reading `allows` with no legacy term beside it must treat
/// unknown as **deny** — see `thread.rs`, where unknown standing must never confer
/// operator reach.
pub async fn load_principal_facts(
    db: &DatabaseConnection,
    caller: &Caller,
) -> Option<PrincipalFacts> {
    load_principal_facts_scoped(db, caller, true).await
}

/// As [`load_principal_facts`], but `include_workspace_facts = false` skips the
/// `workspace_members` query that only the WorkspaceAdmin ring reads.
///
/// This exists for the custom-app data plane, which enforces `WorkspaceDataAccess`
/// on the query hot path (`oxy-customer-apps-perf`): that ring reads the org sets,
/// the platform and partner standings, and the two frontline facts — never the
/// workspace override — so paying for that lookup on every query would be pure
/// waste. One function with a flag, rather than a second loader that could drift
/// from this one.
pub async fn load_principal_facts_scoped(
    db: &DatabaseConnection,
    caller: &Caller,
    include_workspace_facts: bool,
) -> Option<PrincipalFacts> {
    if caller.is_service_account() {
        return Some(service_account_facts(caller));
    }
    // A sandbox agent token holds no tenant standing (sandbox agent credential
    // design §3.2), so none is loaded — not "loaded and dropped". Its facts are
    // its minter's platform standing, bounded to the orgs of the apps it names.
    if caller.is_sandbox_agent() {
        return load_platform_facts(db, caller).await;
    }
    let user_id = caller.user_id;
    // One query for every org membership; the org sets and the partner check both read
    // from these rows instead of re-querying per membership.
    //
    // `Ok(vec![])` (belongs to nothing) and `Err(_)` (we couldn't ask) are different
    // facts and must not become the same empty Vec — conflating them is what turned a
    // blip into a 403.
    let memberships = OrgMembers::find()
        .filter(entity::org_members::Column::UserId.eq(user_id))
        .all(db)
        .await
        .map_err(|e| {
            tracing::warn!(
                target: "authz",
                error = %e,
                user = %user_id,
                "org membership lookup failed — facts are unknown, not empty"
            );
        })
        .ok()?;

    let (owned_orgs, admin_orgs, member_orgs) = derive_org_roles(&memberships);
    // Read the platform sources ONCE: the partner step needs the staff verdict to decide
    // whether to look for assume sessions at all, and the facts need the flags. This
    // runs on the custom-app query hot path.
    //
    // Read for the CALLER, so a token that does not carry platform standing loads
    // none, and one narrowed to a list of orgs loads it over those orgs only.
    let standing = globals::platform_standing_checked(db, caller).await?;
    // NOTE: partner standings still collapse a failed query to "no standing" inside
    // `partner_authz` (operated_partners / standings_for). Same class as the bug this
    // Option fixes — a blip can still cost a partner their reach — but surfacing it
    // means threading fallibility through `resolve_scope`, whose 404-vs-403 existence
    // hiding has to be decided rather than mechanically rewritten. So `Some` here means
    // the loader's OWN reads succeeded, not that every fact in it is known-good.
    let partners = if caller.carries_partner() {
        load_partner_standings(db, &memberships, caller, standing.flags.is_staff()).await
    } else {
        // A token without `partner` holds no partner standing: nothing to load.
        Vec::new()
    };
    let ws_admin_override = if include_workspace_facts {
        let elevated = load_ws_admin_override(db, user_id).await?;
        covered_elevations(db, caller, elevated).await?
    } else {
        Vec::new()
    };
    // Loaded on BOTH paths, unlike the workspace override: AppAccess itself reads
    // these once an app is restricted, and AppAccess is exactly what the scoped
    // (custom-app hot path) caller enforces. Skipping it there would deny an app
    // member their own app.
    let (app_memberships, app_admin_memberships) = load_app_memberships(db, user_id).await?;
    // Same both-paths reasoning as the app memberships above. `?` rather than a
    // default: an unreadable standing is UNKNOWN, and collapsing unknown to
    // "not frontline" would deny a worker mid-shift over a database blip.
    let frontline_orgs = load_frontline_orgs(db, user_id).await?;
    // Only a worker can use this fact, so only a worker pays for the join. For
    // everyone else it is an empty Vec, and `Ring::WorkspaceData` never reads it
    // without frontline standing beside it anyway.
    let frontline_workspace_grants = if frontline_orgs.is_empty() {
        Vec::new()
    } else {
        load_frontline_workspace_grants(db, user_id).await?
    };

    let facts = PrincipalFacts {
        user_id,
        owned_orgs,
        admin_orgs,
        member_orgs,
        partners,
        ws_admin_override,
        app_memberships,
        app_admin_memberships,
        frontline_orgs,
        frontline_workspace_grants,
        service_account: None,
        platform: standing.grant,
        is_global_owner: standing.flags.is_global_owner,
        token: None,
    };
    // The cap at the source. A no-op for a session and for a legacy key.
    Some(caller.narrow(facts))
}

/// The facts of a **service account** (API-tokens design §3.3) — the loader's other
/// branch, and it reads none of the tables the person's branch does.
///
/// A service account's standing is its `service_accounts` row and nothing else: it has
/// no `org_members` row, no email for a platform grant to be keyed by, no partner
/// membership, no workspace elevation, no app grant, no frontline enrolment. So none of
/// those is loaded — not "loaded and found empty" — and a row someone later adds to any
/// of them cannot reach a service account through here. The one fact is the standing
/// the credential carries, which authentication read from the account's row for this
/// very request (a disabled or deleted account never gets this far).
///
/// The token's grants then cap it, as they cap anyone: [`Caller::narrow`].
fn service_account_facts(caller: &Caller) -> PrincipalFacts {
    let service_account = caller
        .account_standing()
        .map(|standing| ServiceAccountStanding {
            org_id: standing.org_id,
            admin: standing.admin,
        });
    caller.narrow(PrincipalFacts {
        user_id: caller.user_id,
        service_account,
        ..Default::default()
    })
}

/// The workspace elevations a narrowing token still carries: those in a workspace a
/// grant covers at `admin` or above. `ws_admin_override` is keyed by workspace with no
/// org beside it, and a grant may be org-wide, so the org is looked up — one query, and
/// only for a token that narrows with elevations to check.
///
/// `None` = the lookup errored: unknown, not empty, like every other fact here.
async fn covered_elevations(
    db: &DatabaseConnection,
    caller: &Caller,
    elevated: Vec<Uuid>,
) -> Option<Vec<Uuid>> {
    let Some(reach) = caller.reach().filter(|r| !r.all_access) else {
        return Some(elevated);
    };
    if elevated.is_empty() {
        return Some(elevated);
    }
    let rows = Workspaces::find()
        .filter(entity::workspaces::Column::Id.is_in(elevated))
        .all(db)
        .await
        .map_err(|e| {
            tracing::warn!(
                target: "authz",
                error = %e,
                user = %caller.user_id,
                "workspace org lookup failed — facts are unknown, not empty"
            );
        })
        .ok()?;
    Some(
        rows.into_iter()
            .filter(|ws| {
                ws.org_id
                    .and_then(|org| reach.workspace_ceiling(org, ws.id))
                    .is_some_and(|c| c >= RoleCeiling::Admin)
            })
            .map(|ws| ws.id)
            .collect(),
    )
}

/// Facts for a PLATFORM decision (Oxy's operator surfaces): the platform grant only.
///
/// The platform rings read nothing else — no org set can reach the `Platform`
/// singleton — so this skips every org / partner / workspace query. An admin route
/// must not pay for tenant facts it never consults. The grant read is itself cached,
/// so this is ~free.
pub async fn load_platform_facts(
    db: &DatabaseConnection,
    caller: &Caller,
) -> Option<PrincipalFacts> {
    // No platform read for a service account: it holds no staff standing by
    // construction, and its facts say so to the model.
    if caller.is_service_account() {
        return Some(service_account_facts(caller));
    }
    let standing = globals::platform_standing_checked(db, caller).await?;
    Some(caller.narrow(PrincipalFacts {
        user_id: caller.user_id,
        platform: standing.grant,
        is_global_owner: standing.flags.is_global_owner,
        ..Default::default()
    }))
}

/// `(owned, admin, member)` org sets from the user's already-loaded `org_members`
/// rows (no query). `owned ⊆ admin ⊆ member` (owner implies admin implies member) so
/// a single role check populates the ring the action needs.
fn derive_org_roles(
    memberships: &[entity::org_members::Model],
) -> (Vec<Uuid>, Vec<Uuid>, Vec<Uuid>) {
    let mut owned = Vec::new();
    let mut admin = Vec::new();
    let mut member = Vec::new();
    for m in memberships {
        member.push(m.org_id);
        if matches!(m.role, OrgRole::Owner | OrgRole::Admin) {
            admin.push(m.org_id);
        }
        if matches!(m.role, OrgRole::Owner) {
            owned.push(m.org_id);
        }
    }
    (owned, admin, member)
}

/// The principal's partner standings, straight from [`partner_authz::operated_partners`]
/// — the same source `resolve_scope` reads.
///
/// This used to hand-roll the operator + ceiling queries and carry a "keep these two in
/// sync" comment. That was the drift-between-siblings failure this whole layer exists to
/// remove, re-created by hand and left for a human to police. Now there is one
/// implementation; the loader just reshapes it into facts.
async fn load_partner_standings(
    db: &DatabaseConnection,
    memberships: &[entity::org_members::Model],
    caller: &Caller,
    is_staff: bool,
) -> Vec<PartnerStanding> {
    // Real operators, plus any partner this user is standing in through a live
    // assume-role session. The assumed half is what the model used to be blind to — the
    // console's scope knew about the override and the facts didn't, so the unified rings
    // would have denied staff the console. That gap is why partner_policy could not
    // retire. Non-staff short-circuit before any query.
    let mut standings = partner_authz::operated_partners(db, memberships).await;
    let assumed = partner_authz::assumed_partners(db, caller, is_staff).await;
    for a in assumed {
        // A real operator's standing wins; they are the same ceiling anyway.
        if !standings.iter().any(|p| p.partner_id == a.partner_id) {
            standings.push(a);
        }
    }
    standings
        .into_iter()
        .map(|p| PartnerStanding {
            partner_id: p.partner_id,
            client_orgs: p.managed_org_ids,
            caps: caps_of(&p.capabilities),
        })
        .collect()
}

/// Translate the partner tier's ceiling into the authz vocabulary. One mapping, and the
/// compiler sees every arm of it.
fn caps_of(c: &partner_authz::Capabilities) -> Vec<Cap> {
    let mut caps = Vec::new();
    for (on, cap) in [
        (c.manage_members, Cap::ManageMembers),
        (c.manage_apps, Cap::ManageApps),
        (c.develop_apps, Cap::DevelopApps),
        (c.view_audit, Cap::ViewAudit),
        (c.manage_billing, Cap::ManageBilling),
        (c.manage_secrets, Cap::ManageSecrets),
        (c.create_orgs, Cap::CreateOrgs),
        (c.manage_org_settings, Cap::ManageOrgSettings),
    ] {
        if on {
            caps.push(cap);
        }
    }
    caps
}

/// Apps the user holds a grant on, as `(every app, admin apps)`. `admin ⊆ all`,
/// mirroring how the org sets nest so a ring reads one set rather than re-deriving
/// the role.
///
/// A grant reaches the user two ways, and this is the ONLY place that difference
/// exists:
///
/// - **directly** — an `app_members` row naming them, and
/// - **through a team** — an `org_team_members` row putting them in a team that an
///   `app_team_grants` row grants the app to.
///
/// Both land in the same two vectors, which is why no `oxy-authz` ring mentions
/// teams: to the model a grant is a grant, and teams are just a second way for the
/// fact to be true. `admin` is the union of both admin sources, so a plain direct
/// row plus an admin team grant reads as admin (the strongest grant wins — the same
/// way two org memberships would).
///
/// Two bounded queries on indexed columns (`app_members.user_id`, then
/// `org_team_members.user_id` joined to `app_team_grants.team_id`).
///
/// `None` = a query errored. Same reasoning as the org sets and the workspace
/// override: a member of a RESTRICTED app whose lookup blips is not "a user with no
/// app membership" — reporting them as one costs them the app entirely.
async fn load_app_memberships(
    db: &DatabaseConnection,
    user_id: Uuid,
) -> Option<(Vec<Uuid>, Vec<Uuid>)> {
    let rows = entity::prelude::AppMembers::find()
        .filter(entity::app_members::Column::UserId.eq(user_id))
        .all(db)
        .await
        .map_err(|e| {
            tracing::warn!(
                target: "authz",
                error = %e,
                user = %user_id,
                "app membership lookup failed — facts are unknown, not empty"
            );
        })
        .ok()?;
    let mut all = Vec::with_capacity(rows.len());
    let mut admins = Vec::new();
    for row in rows {
        if row.is_admin() {
            admins.push(row.app_id);
        }
        all.push(row.app_id);
    }

    for grant in load_team_grants(db, user_id).await? {
        if grant.is_admin() && !admins.contains(&grant.app_id) {
            admins.push(grant.app_id);
        }
        if !all.contains(&grant.app_id) {
            all.push(grant.app_id);
        }
    }

    Some((all, admins))
}

/// Orgs where this user is an **active** frontline worker.
///
/// Loaded on both paths, like `load_app_memberships` and for the same reason:
/// the rings that read it — `AppAccess` for the app, `WorkspaceData` for the
/// data plane behind it — are exactly what the scoped (custom-app hot path)
/// callers enforce. Skipping it there would deny a frontline worker the app they
/// were enrolled to use.
///
/// `status = 'active'` is in the QUERY, not a later filter. A suspended worker
/// must produce no fact at all — a row that reaches [`allows`] and is discarded
/// downstream is one refactor away from being read as standing.
async fn load_frontline_orgs(db: &DatabaseConnection, user_id: Uuid) -> Option<Vec<Uuid>> {
    let rows = entity::prelude::OrgFrontlineMembers::find()
        .filter(entity::org_frontline_members::Column::UserId.eq(user_id))
        .filter(
            entity::org_frontline_members::Column::Status
                .eq(entity::org_frontline_members::STATUS_ACTIVE),
        )
        .all(db)
        .await
        .map_err(|e| {
            tracing::warn!(
                target: "authz",
                error = %e,
                user = %user_id,
                "frontline standing lookup failed — facts are unknown, not empty"
            );
        })
        .ok()?;
    Some(rows.into_iter().map(|r| r.org_id).collect())
}

/// Workspaces from which an app this user holds an `app_members` row on was
/// published — the fact `Ring::WorkspaceData` reads for a frontline worker.
///
/// The custom-app data plane is keyed by workspace and a worker's grant by app,
/// and this is the join between them, derived ONCE here rather than re-decided
/// in the gate. `check_custom_app_gates` used to hand the ring a workspace id as
/// if it were an app id, so the ring could never see a worker's grant and the
/// gate grew an exemption that skipped the model for them; this fact is what
/// lets the conjunction hold instead.
///
/// Direct rows only. A team grant cannot reach a worker — `add_team_member`
/// rejects anyone without an `org_members` row, which a worker never holds — so
/// the union [`load_app_memberships`] performs is empty here by construction.
/// If team membership is ever opened to workers, this is the second place that
/// must learn about it (the first is `frontline_worker_with_app_grant`).
///
/// `None` = the query errored: unknown, not empty, for the same reason as every
/// other set here.
async fn load_frontline_workspace_grants(
    db: &DatabaseConnection,
    user_id: Uuid,
) -> Option<Vec<Uuid>> {
    use sea_orm::{JoinType, QuerySelect, RelationTrait};
    AppMembers::find()
        .select_only()
        .column(entity::apps::Column::ProjectId)
        .distinct()
        .join(
            JoinType::InnerJoin,
            entity::app_members::Relation::Apps.def(),
        )
        .filter(entity::app_members::Column::UserId.eq(user_id))
        // Published apps only, as `user_can_access_app` requires of every
        // customer — a grant on a draft is not reach into the workspace.
        .filter(entity::apps::Column::PublishedAt.is_not_null())
        .into_tuple::<Uuid>()
        .all(db)
        .await
        .map_err(|e| {
            tracing::warn!(
                target: "authz",
                error = %e,
                user = %user_id,
                "frontline workspace grant lookup failed — facts are unknown, not empty"
            );
        })
        .ok()
}

/// Every `app_team_grants` row reachable from the user's team memberships.
///
/// Split from [`load_app_memberships`] only to keep each function to one job; it is
/// not separately callable by a ring. Returns an empty vec (not `None`) when the user
/// is in no teams — that is a known fact, unlike a failed query.
async fn load_team_grants(
    db: &DatabaseConnection,
    user_id: Uuid,
) -> Option<Vec<entity::app_team_grants::Model>> {
    let team_ids: Vec<Uuid> = entity::prelude::OrgTeamMembers::find()
        .filter(entity::org_team_members::Column::UserId.eq(user_id))
        .all(db)
        .await
        .map_err(|e| {
            tracing::warn!(
                target: "authz",
                error = %e,
                user = %user_id,
                "team membership lookup failed — facts are unknown, not empty"
            );
        })
        .ok()?
        .into_iter()
        .map(|row| row.team_id)
        .collect();
    if team_ids.is_empty() {
        return Some(Vec::new());
    }

    entity::prelude::AppTeamGrants::find()
        .filter(entity::app_team_grants::Column::TeamId.is_in(team_ids))
        .all(db)
        .await
        .map_err(|e| {
            tracing::warn!(
                target: "authz",
                error = %e,
                user = %user_id,
                "team app-grant lookup failed — facts are unknown, not empty"
            );
        })
        .ok()
}

/// Workspaces where a `workspace_members` override raises the user to Admin or Owner.
/// Only these exceptional rows are loaded — the org-derived workspace role comes free
/// from the org sets via the hierarchy, and overrides can only elevate. One bounded
/// query.
///
/// `None` = the query errored. Same reasoning as the org sets: an elevated member whose
/// override lookup blips is not "a member with no override", and reporting them as one
/// costs them the workspace their `workspace_members` row grants.
async fn load_ws_admin_override(db: &DatabaseConnection, user_id: Uuid) -> Option<Vec<Uuid>> {
    let rows = WorkspaceMembers::find()
        .filter(entity::workspace_members::Column::UserId.eq(user_id))
        .all(db)
        .await
        .map_err(|e| {
            tracing::warn!(
                target: "authz",
                error = %e,
                user = %user_id,
                "workspace override lookup failed — facts are unknown, not empty"
            );
        })
        .ok()?;
    Some(
        rows.into_iter()
            .filter(|m| {
                matches!(
                    m.role,
                    entity::workspace_members::WorkspaceRole::Owner
                        | entity::workspace_members::WorkspaceRole::Admin
                )
            })
            .map(|m| m.workspace_id)
            .collect(),
    )
}
