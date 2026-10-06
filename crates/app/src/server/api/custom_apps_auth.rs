//! Access control for custom apps.
//!
//! Two independent grants compose to "can this user reach this app":
//!
//! 1. **Org membership** — the historical check. A member of the owning
//!    org sees every app in that org.
//! 2. **Oxy staff** — the caller is a member of `app_admins` (an Oxy-staff
//!    role managed by `OXY_OWNER` users) AND the workspace has NOT locked
//!    Oxy out (no row in `workspace_oxy_lockdown`). Staff access is the
//!    DEFAULT (inverted 2026-07-14 — the old opt-in consent row was
//!    self-grantable by staff, so it protected nobody); an org officer can
//!    revoke it at any time with the lockdown switch.
//!
//! The combined check is fronted by a `(user_id, app_id) → bool` cache
//! so a Next.js page load's asset storm doesn't hit the DB three times
//! per chunk. Cache TTL matches the existing membership cache so a
//! revocation of any source propagates within a minute.
//!
//! All helpers are async because the underlying tables are queried. The
//! email-keyed Global-Admin check (`app_admins`) now lives in
//! `server::authz::globals::is_app_admin_email` — authz owns that read.
//!
//! ## Bundle / manifest helpers
//!
//! Manifest schema, caching, and bundle-dir resolution live in
//! `custom_apps_manifest`. `authenticate_and_authorize` lives here so
//! any handler — not just the debug one — can reuse the full auth
//! pipeline without duplicating the logic.

use axum::http::StatusCode;
use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};
use std::time::Instant;

use entity::prelude::{
    AppAdmins, AppMembers, AppTeamGrants, Apps, OrgMembers, OrgTeamMembers, OrgTeams,
    Organizations, WorkspaceOxyLockdown,
};
use entity::{
    app_admins, app_members, app_team_grants, apps, org_members, org_team_members, org_teams,
    organizations, workspace_oxy_lockdown,
};
use oxy::database::client::establish_connection;
use oxy_auth::authenticator::Authenticator;
use oxy_auth::built_in::BuiltInAuthenticator;
use oxy_auth::user::UserService;
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use tracing::error;
use uuid::Uuid;

use super::custom_apps_cache::{cached_user, get_fresh, insert_with_sweep, set_cached_user};

// ── Combined per-(user, app) access cache ───────────────────────────────────

fn access_cache() -> &'static RwLock<HashMap<(Uuid, Uuid), (bool, Instant)>> {
    static CACHE: OnceLock<RwLock<HashMap<(Uuid, Uuid), (bool, Instant)>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn cached_access(user_id: Uuid, app_id: Uuid) -> Option<bool> {
    get_fresh(access_cache(), &(user_id, app_id))
}

fn set_cached_access(user_id: Uuid, app_id: Uuid, allowed: bool) {
    insert_with_sweep(access_cache(), (user_id, app_id), allowed);
}

/// Called by the admin / oxy-access mutation handlers so a freshly
/// toggled state takes effect immediately instead of waiting out the
/// cache TTL. We don't know which user_id × app_id pairs are affected,
/// so we drop the whole cache.
pub fn invalidate_access_cache() {
    if let Ok(mut guard) = access_cache().write() {
        guard.clear();
    }
}

// ── Primary access check ────────────────────────────────────────────────────

/// True if `user` may serve / read data products for `app`. Two
/// independent access paths:
///
/// - **Oxy staff path**: platform standing is staff — Global Owner **or** Global
///   Admin, read through `oxy_server_authz::globals::platform_standing`, not from
///   `app_admins` directly — plus the workspace has not locked Oxy out
///   (`workspace_oxy_lockdown`). Works on draft (unpublished) apps too, which is
///   how Oxy engineers iterate.
/// - **Customer path**: the app is **published** (`published_at IS NOT NULL`) and
///   then, by visibility:
///   - `org` (the default) — any member of the owning org.
///   - `members` (restricted) — an org member who also holds a grant on the app,
///     direct or through a team ([`has_app_grant`]; a grant narrows the org, it is
///     not a way into one), **or** an org officer, who keeps a break-glass path so
///     an org cannot lock its own owners out of its app.
///
///   Unpublished apps are invisible to customers — they look like 404s.
///
/// The customer path mirrors `Ring::AppAccess` in `oxy-authz`; this is the shipped
/// gate that ring is differenced against, so the two must be changed together.
///
/// Short-circuits on the staff path so an Oxy engineer's request
/// skips the org-membership query, and on the unpublished check so
/// customers don't fan out to membership for draft apps.
///
/// Takes the [`oxy_server_authz::Caller`]: an API token reaches an app only
/// when a grant covers the workspace it was published from, staff standing is
/// what the token carries, and the officer break-glass needs an admin ceiling.
/// Outside the grant the answer is `false` — to a token the app does not exist.
pub async fn user_can_access_app(
    db: &DatabaseConnection,
    caller: &oxy_server_authz::Caller,
    app: &apps::Model,
) -> Result<bool, DbErr> {
    let user_id = caller.user_id;
    let Some(ceiling) = caller.workspace_ceiling(app.org_id, app.project_id) else {
        return Ok(false);
    };
    // The verdict cache is keyed by (user, app) — a session's verdict. A token
    // that narrows its bearer must neither read one nor leave one behind.
    let cacheable = !caller.reach().is_some_and(oxy_authz::TokenReach::narrows);
    if cacheable && let Some(v) = cached_access(user_id, app.id) {
        return Ok(v);
    }

    // Staff path first: works regardless of publish state. Oxy staff may access
    // BY DEFAULT — unless the org has locked them out (inverted 2026-07-14).
    //
    // "Staff" is admin OR owner, read through the one place that knows what the
    // platform sources are. It used to be app-admin ONLY, so a Global Owner who wasn't
    // also in `app_admins` could PUBLISH a custom app (publish resolves staff via
    // platform_standing) but not VIEW one — the same question answered two ways in one
    // subsystem. Both operator tiers reach everything; they separate only at
    // owner-exclusive destructive operations.
    // `develop_apps` over THIS app's org — not a bare `is_staff()`. Every platform role
    // reports staff, and a grant bounded to one org must not open another's app.
    let allowed = if oxy_server_authz::globals::platform_reaches(
        db,
        caller,
        oxy_authz::Cap::DevelopApps,
        app.org_id,
    )
    .await
        && !is_oxy_locked_down(db, app.project_id).await?
    {
        true
    } else if app.published_at.is_some() {
        // Customer path: only if published — and, for a RESTRICTED app
        // (`visibility = 'members'`), org membership alone is no longer enough.
        // An org officer (owner/admin) keeps a break-glass path so an org can't
        // lock its own staff out of its app. Mirrors `Ring::AppAccess` in
        // `oxy-authz`, which states the same rule; this is the shipped gate that
        // ring is differenced against.
        let member = if app.is_restricted() {
            // A grant NARROWS the org — it is not a way into one. Without the
            // org-membership conjunction, a grant row let a non-member load the
            // app's shell while `check_custom_app_gates` (which requires org
            // membership) 403'd every query behind it. Mirrors the same term in
            // `Ring::AppAccess`.
            (is_org_member(db, user_id, app.org_id).await?
                && has_app_grant(db, user_id, app.id).await?)
                || (ceiling >= oxy_authz::RoleCeiling::Admin
                    && is_org_officer(db, user_id, app.org_id).await?)
        } else {
            is_org_member(db, user_id, app.org_id).await?
        };
        // The crew. A frontline worker is not an org member and never will be —
        // `enroll_worker` writes `users.email = NULL` to keep them out of every
        // email-keyed path — so without this term every branch above is false for
        // them and the app they were enrolled to use answered 403 to its shell
        // and to every function behind it. The data-plane gate admitted them and
        // the model (`Ring::AppAccess`'s frontline term) admitted them; this, the
        // gate every app-keyed surface calls, was the one that did not.
        //
        // Standing AND a grant, restricted or not: an org-visible app is visible
        // to org MEMBERS, and a worker is deliberately not one. Standing first —
        // a primary-key read on `org_frontline_members`, false for everyone who
        // is not a worker, so the grant lookup runs only for the crew.
        member
            || (oxy_auth::frontline::is_active_frontline(db, app.org_id, user_id)
                .await
                .map_err(|e| DbErr::Custom(e.to_string()))?
                && has_app_grant(db, user_id, app.id).await?)
    } else {
        false
    };

    if cacheable {
        set_cached_access(user_id, app.id, allowed);
    }
    Ok(allowed)
}

/// Whether `user_id` holds any grant on `app_id`, **direct or through a team**.
///
/// The ONE place the two grant kinds are unioned on the shipped-gate side; the fact
/// loader does the same union for `oxy-authz`. Anything asking "does this user have a
/// grant" must come through here, or the two sources drift.
///
/// Deliberately an existence check and not a role resolution. Both callers only need
/// "is there a grant": [`user_can_access_app`] gates access on it, and
/// [`resolve_app_role`] reports `member` for anyone who isn't already `admin` — and
/// `admin` comes from `Ring::AppAdmin`, which reads the loader's unioned
/// `app_admin_memberships`. A second strongest-grant-wins resolution here would be a
/// parallel copy of a rule the model already owns, which is exactly the drift
/// `oxy-authz` exists to end. The strongest-wins property is pinned where it lives,
/// in the loader (`authz_loader_differential`).
///
/// Short-circuits on the direct row so the common case costs one query.
pub async fn has_app_grant(
    db: &DatabaseConnection,
    user_id: Uuid,
    app_id: Uuid,
) -> Result<bool, DbErr> {
    let direct = AppMembers::find()
        .filter(app_members::Column::AppId.eq(app_id))
        .filter(app_members::Column::UserId.eq(user_id))
        .one(db)
        .await?;
    if direct.is_some() {
        return Ok(true);
    }

    let team_ids: Vec<Uuid> = OrgTeamMembers::find()
        .filter(org_team_members::Column::UserId.eq(user_id))
        .all(db)
        .await?
        .into_iter()
        .map(|row| row.team_id)
        .collect();
    if team_ids.is_empty() {
        return Ok(false);
    }
    AppTeamGrants::find()
        .filter(app_team_grants::Column::AppId.eq(app_id))
        .filter(app_team_grants::Column::TeamId.is_in(team_ids))
        .one(db)
        .await
        .map(|row| row.is_some())
}

/// Returns `true` when `user_id` is an **officer** (owner or admin) of `org_id`.
/// The break-glass term for restricted-app access: an org's own officers are never
/// locked out of its apps. (`.is_in` rather than the `Owner | Admin` match literal
/// the authz-boundary guard bans in handlers.)
pub(crate) async fn is_org_officer(
    db: &DatabaseConnection,
    user_id: Uuid,
    org_id: Uuid,
) -> Result<bool, DbErr> {
    OrgMembers::find()
        .filter(org_members::Column::UserId.eq(user_id))
        .filter(org_members::Column::OrgId.eq(org_id))
        .filter(
            org_members::Column::Role
                .is_in([org_members::OrgRole::Owner, org_members::OrgRole::Admin]),
        )
        .one(db)
        .await
        .map(|opt| opt.is_some())
}

/// The invoking user's role **within this app**, as surfaced to a function through
/// `ctx.user.appRole`.
///
/// `Some("admin")` when they administer it — any org **officer** (owner or admin),
/// an `app_members` admin row, or Oxy staff. `Some("member")` for a plain
/// `app_members` row, `None` otherwise. A plain org member is NOT an admin unless
/// granted the row — that is the line the model draws (officer, not member), and the
/// `app_members` admin role is how a non-officer becomes one.
///
/// This mirrors `Ring::AppAdmin` in `oxy-authz`; a function that gates a privileged
/// surface on it is server-enforcing, not merely hiding a tab.
pub async fn resolve_app_role(
    db: &DatabaseConnection,
    caller: &oxy_server_authz::Caller,
    app: &apps::Model,
) -> Result<Option<&'static str>, DbErr> {
    let user_id = caller.user_id;
    // The admin verdict comes from the ONE model — not a second copy of the rule
    // written out here. Restating "staff OR org owner OR app-admin row" in SQL is
    // exactly the drift `oxy-authz` exists to end, and it would silently diverge
    // the moment `Ring::AppAdmin` changed.
    //
    // Workspace facts are skipped: no app ring reads them.
    //
    // The resource names the workspace the app was published from, so an API
    // token is an app admin only where its ceiling over THAT workspace reaches
    // admin — the per-app admin row included. Under a lower ceiling the caller
    // falls through to `member` below: the capped role, as for any other.
    let resource =
        oxy_authz::Resource::app_with_visibility(app.id, app.org_id, app.is_restricted())
            .published_from(app.project_id);
    let is_admin =
        match oxy_server_authz::loader::load_principal_facts_scoped(db, caller, false).await {
            Some(facts) => oxy_authz::allows(&facts, oxy_authz::Action::AppAdmin, &resource),
            // Facts unknown (a DB blip) → not admin. Fail closed.
            None => false,
        };
    if is_admin {
        return Ok(Some(app_members::ROLE_ADMIN));
    }
    // Not an admin — a plain grant still reports as "member" so an app can
    // distinguish "belongs to this app" from "just any org member". Goes through
    // `has_app_grant` so a team-granted user isn't reported as `None`, which would
    // make team grants invisible to `ctx.user.appRole`.
    Ok(has_app_grant(db, user_id, app.id)
        .await?
        .then_some(app_members::ROLE_MEMBER))
}

/// The invoking user's role in the **owning org**, as surfaced through
/// `ctx.user.orgRole` — `"owner"`, `"admin"`, `"member"`, or `None` when they
/// reach the app without an org membership (Oxy staff on break-glass, a partner
/// operating downstream).
///
/// This is a **fact read**, not an authorization decision: it reports the row in
/// `org_members` verbatim and decides nothing. That is what keeps it clear of the
/// rule the authz-boundary test enforces — no handler may branch on
/// `matches!(role, Owner | Admin)`. An app that wants to *gate* asks
/// [`resolve_app_role`], which goes through `oxy-authz`; this one exists so an app
/// can explain itself ("ask your org admin"), label a byline, or route a
/// tenant-admin view. Deliberately not cached: a stale role is worse than a query.
///
/// Callers that also want the caller's teams should use [`resolve_org_standing`],
/// which returns both for this one read — teams are gated on this same
/// membership, so asking for them separately would read the row twice.
pub async fn resolve_org_role(
    db: &DatabaseConnection,
    user_id: Uuid,
    org_id: Uuid,
) -> Result<Option<&'static str>, DbErr> {
    Ok(OrgMembers::find()
        .filter(org_members::Column::UserId.eq(user_id))
        .filter(org_members::Column::OrgId.eq(org_id))
        .one(db)
        .await?
        .map(|row| row.role.as_str()))
}

/// Both org-level identity facts — role and teams — for **one** membership read.
///
/// The role lookup *is* the teams gate, so resolving them independently reads
/// `org_members` twice. Worth avoiding here specifically: unlike the view
/// recorder, the function-invocation path runs these synchronously *before* the
/// isolate starts, so a redundant round trip is latency a caller waits on.
///
/// **The only way to ask for teams.** A teams-only variant existed briefly and
/// wrote the membership gate a second time; nothing shipped called it, so the
/// tenant-boundary and stale-row tests guarded a copy of the rule rather than the
/// rule. One entry point means the gate is stated once and the tests exercise the
/// path that actually runs.
pub async fn resolve_org_standing(
    db: &DatabaseConnection,
    user_id: Uuid,
    org_id: Uuid,
) -> Result<(Option<&'static str>, Vec<OrgTeamRef>), DbErr> {
    let Some(role) = resolve_org_role(db, user_id, org_id).await? else {
        // Not a member: no role, and no teams to report even if stale rows exist.
        return Ok((None, Vec::new()));
    };
    Ok((Some(role), org_teams_for_member(db, user_id, org_id).await?))
}

/// The joined team read, for a caller that has already established membership.
///
/// **One query, scoped in the join** rather than "fetch every team row this user
/// has anywhere, then narrow". The result is the same either way; the difference
/// is that the `IN (…)` list in the two-query form scales with the user's
/// *cross-tenant* footprint — precisely the consultant-in-six-orgs case the org
/// scoping exists for. Structural scoping also can't be dropped by a later edit
/// the way a trailing `.filter` can.
///
/// Sorted case-insensitively. A byte-order sort puts every lowercase name after
/// every capitalised one, and team names are user-authored free text rendered in
/// an app's UI, where `["Finance", "Store Managers", "aardvarks"]` reads as
/// unsorted. Ties fall back to byte order so two names differing only in case
/// still order deterministically.
async fn org_teams_for_member(
    db: &DatabaseConnection,
    user_id: Uuid,
    org_id: Uuid,
) -> Result<Vec<OrgTeamRef>, DbErr> {
    let mut teams: Vec<OrgTeamRef> = OrgTeamMembers::find()
        .filter(org_team_members::Column::UserId.eq(user_id))
        .find_also_related(OrgTeams)
        .filter(org_teams::Column::OrgId.eq(org_id))
        .all(db)
        .await?
        .into_iter()
        .filter_map(|(_, team)| team)
        .map(|team| OrgTeamRef {
            id: team.id,
            name: team.name,
        })
        .collect();
    teams.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(teams)
}

/// One org team, flattened for [`resolve_org_standing`]. Deliberately not the entity
/// model: `ctx.user.teams` is a public wire shape handed to third-party app code,
/// so it exposes id + name and nothing else the table may grow later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgTeamRef {
    pub id: Uuid,
    pub name: String,
}

/// Returns `true` when `user_id` is a member of `org_id`.
///
/// Does **not** cache — callers that need caching should go through
/// [`user_can_access_app`] instead.
pub(crate) async fn is_org_member(
    db: &DatabaseConnection,
    user_id: Uuid,
    org_id: Uuid,
) -> Result<bool, DbErr> {
    OrgMembers::find()
        .filter(org_members::Column::UserId.eq(user_id))
        .filter(org_members::Column::OrgId.eq(org_id))
        .one(db)
        .await
        .map(|opt| opt.is_some())
}

/// Whether this workspace has **locked Oxy staff out**. A row in
/// `workspace_oxy_lockdown` is the toggle.
///
/// Staff access is the default (inverted 2026-07-14 — the old opt-in consent row
/// was self-grantable by staff, so it protected nobody), and this is how an org
/// officer revokes it. Read as a conjunction with the staff verdict in
/// [`user_can_access_app`]: staff reach an app only while their org has not
/// locked them out.
///
/// (Comment repaired 2026-08-21: two functions' docs had been welded together by
/// a comment-stripping sweep, leaving a sentence about a "build tailored apps"
/// consent toggle that no longer exists spliced onto this one.)
pub async fn is_oxy_locked_down(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> Result<bool, DbErr> {
    WorkspaceOxyLockdown::find()
        .filter(workspace_oxy_lockdown::Column::WorkspaceId.eq(workspace_id))
        .one(db)
        .await
        .map(|opt| opt.is_some())
}

/// Convenience: load an app by `(org_slug, app_slug)`. Used by the few
/// callers that need both the access check and the app row but don't
/// already have the app in hand.
pub async fn load_app_by_slugs(
    db: &DatabaseConnection,
    org_id: Uuid,
    app_slug: &str,
) -> Result<Option<apps::Model>, DbErr> {
    Apps::find()
        .filter(apps::Column::OrgId.eq(org_id))
        .filter(apps::Column::Slug.eq(app_slug))
        .one(db)
        .await
}

// ── Auth helper ──────────────────────────────────────────────────────────────

/// Narrow an [`AuthOutcome`] from "may open this app" to "may operate it".
///
/// [`authenticate_and_authorize`] ends at [`user_can_access_app`], which for a
/// default-visibility published app is true for **every member of the owning
/// org** — the app's ordinary viewers. That is the right gate for the bundle's
/// own bytes. It is the wrong gate for anything that exposes the app's
/// *operator-facing* internals: server-side `ctx.log()` output (which routinely
/// carries query results and upstream API responses the author printed while
/// debugging), and client stacks resolved against source maps (original file
/// paths and function names). An app's data is what it chose to show a viewer;
/// its log output is not, and "the same gate the app's own data already has" is
/// not a defence for the second one.
///
/// The verdict comes from `oxy-authz`'s `Ring::AppAdmin` via
/// [`resolve_app_role`] — never a hand-rolled staff/owner check here — so it
/// already covers Oxy staff, the org owner and an app-admin row.
pub(crate) async fn require_app_admin(outcome: &AuthOutcome) -> Result<(), StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        error!("db connect failed for app-admin check: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let role = resolve_app_role(&db, &outcome.caller, &outcome.app)
        .await
        .map_err(|e| {
            error!("app role lookup failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    if role == Some(app_members::ROLE_ADMIN) {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

/// What the auth flow returns on success.
pub(crate) struct AuthOutcome {
    pub app: apps::Model,
    pub user_id: Uuid,
    /// `None` for a frontline worker enrolled without a mailbox. Reaches an
    /// app as `ctx.user.email`, so an app that assumes a string gets `null`
    /// rather than `""` — the difference between "has no address" and "has an
    /// address that is empty" is exactly what a notification feature needs.
    pub user_email: Option<String>,
    /// `users.name`. Display identity, carried so a function reading
    /// `ctx.user.name` doesn't have to take the client's word for it — the whole
    /// value of server-side identity is that it can't be spoofed by the caller.
    pub user_name: String,
    /// `users.picture`, when they have one.
    pub user_picture: Option<String>,
    pub is_staff: bool,
    /// The user with the credential the request arrived with — what every
    /// further authorization question about this request must be asked of, so
    /// an API token's grants and standing flags hold on the custom-app paths too.
    pub caller: oxy_server_authz::Caller,
}

/// The key the per-request user cache is kept under: **who the credential
/// names**, falling back to the address it carries only when it names nobody.
///
/// It was the lowercased email string. A session JWT resolves by `sub`, not by
/// its email claim, and a frontline worker's claim is `""` — `users.email` is
/// NULL for the crew — so every worker in every org shared ONE cache slot. The
/// first worker to invoke a function was handed back for every kiosk request
/// in the next 60 seconds: Sam's PIN, Maria's `ctx.user.id`, name and
/// `appRole`, in every log line and audit row. Invisible while
/// [`user_can_access_app`] refused workers outright; the day it admitted them
/// this became the first thing they hit.
///
/// A provider identity (Google, Okta, magic link) has no id yet and one real
/// address, so the address is the right key there and stays so.
pub(crate) fn user_cache_key(identity: &oxy_auth::types::Identity) -> String {
    match identity.user_id {
        Some(id) => id.to_string(),
        None => identity.email.to_ascii_lowercase(),
    }
}

/// Authenticate the request and confirm the caller has access to
/// (org, app). Returns the app row + user info on success; an HTTP
/// status on any failure.
///
/// `sandbox_agent` is this caller's answer to a sandbox agent token. `/fn` and
/// `/logs` admit one; `/errors`, `/debug`, `/health` and `/availability` share
/// this function and refuse it, which is why the answer is an argument and not
/// a property of the function (sandbox agent credential design §3.1).
pub(crate) async fn authenticate_and_authorize(
    headers: &axum::http::HeaderMap,
    org_slug: &str,
    app_slug: &str,
    sandbox_agent: oxy_auth::token::SandboxAgent,
) -> Result<AuthOutcome, axum::http::StatusCode> {
    use axum::http::StatusCode;

    let (identity, credential) = BuiltInAuthenticator::new(sandbox_agent)
        .authenticate_with_credential(headers)
        .await
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    // A sandbox agent token: noted for the request's usage count, and its
    // minter's row read now, past the 60 s user cache in both directions, so a
    // change to the minter is seen by the next request (`custom_apps_agent`).
    super::custom_apps_agent::seen(credential.as_ref());
    let cache_key = user_cache_key(&identity);
    let user = if super::custom_apps_agent::is_agent(credential.as_ref()) {
        // Boxed, so the token's branch adds a pointer to this future, which the
        // function route awaits inline.
        Box::pin(super::custom_apps_agent::fresh_user(&identity))
            .await
            .map_err(|e| {
                error!("user lookup failed: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            })?
            .ok_or(StatusCode::UNAUTHORIZED)?
    } else if let Some(u) = cached_user(&cache_key) {
        u
    } else {
        let u = UserService::find_user_by_identity(&identity)
            .await
            .map_err(|e| {
                error!("user lookup failed: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            })?
            .ok_or(StatusCode::UNAUTHORIZED)?;
        set_cached_user(cache_key, u.clone());
        u
    };
    // Attached AFTER the cache: the cached row is the user, never the credential
    // one request happened to arrive with.
    let user = user.with_credential(credential);
    let caller = oxy_server_authz::Caller::from_user(&user);

    let db = establish_connection().await.map_err(|e| {
        error!("DB connection failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let org = Organizations::find()
        .filter(organizations::Column::Slug.eq(org_slug))
        .one(&db)
        .await
        .map_err(|e| {
            error!("org lookup failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    let app = Apps::find()
        .filter(apps::Column::OrgId.eq(org.id))
        .filter(apps::Column::Slug.eq(app_slug))
        .one(&db)
        .await
        .map_err(|e| {
            error!("app lookup failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    // An API token with no grant on the app's workspace answers exactly as an
    // unknown app does, so it cannot probe which apps exist.
    if caller
        .workspace_ceiling(app.org_id, app.project_id)
        .is_none()
    {
        return Err(StatusCode::NOT_FOUND);
    }
    // A sandbox agent token's workspace grant stands for the apps it names,
    // not for every app of the workspace: any other is an unknown app.
    if !super::custom_apps_agent::admits_app(&caller, &app) {
        return Err(StatusCode::NOT_FOUND);
    }

    let allowed = user_can_access_app(&db, &caller, &app).await.map_err(|e| {
        error!("access check failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    if !allowed {
        return Err(StatusCode::FORBIDDEN);
    }

    // Same definition as the access check above — otherwise an owner would pass access
    // but be flagged a customer, and silently lose draft previews. It must stay
    // literally the same call, capability and scope included: the two drifting apart is
    // precisely the bug this comment was written for.
    let is_staff = oxy_server_authz::globals::platform_reaches(
        &db,
        &caller,
        oxy_authz::Cap::DevelopApps,
        app.org_id,
    )
    .await;

    Ok(AuthOutcome {
        app,
        user_id: user.id,
        user_email: user.email,
        user_name: user.name,
        user_picture: user.picture,
        is_staff,
        caller,
    })
}

// ── Bootstrap: OXY_GLOBAL_ADMINS env → app_admins table ─────────────────────

/// The pre-rename `OXY_APP_ADMINS` is gone from every reader (seeding, the
/// `seed` command, dev sign-in). Removing a var that used to grant staff
/// access fails silently by nature — nothing errors, there are simply no
/// admins — so say it loudly at the moment the seed would have used it.
///
/// Keyed on **which emails are lost**, not on whether the variable is set.
/// The old code unioned the two lists, so "both set" also covers a
/// half-migrated deployment whose lists are disjoint — some staff under the new
/// name, others still only under the old one. That is precisely where addresses
/// silently stop being seeded, and a set-vs-unset rule is silent for it. The
/// bite is bounded (seeding is insert-only, so rows already created survive) but
/// lands on a fresh database, or on anyone never seeded, as "why isn't X an
/// admin" with nothing in the log.
///
/// A fully-migrated deployment that just left the old line behind stays quiet,
/// which is what the set-vs-unset rule was reaching for.
pub(crate) fn warn_on_removed_legacy_admins_env() {
    let lost = legacy_only_emails(
        std::env::var("OXY_APP_ADMINS").ok().as_deref(),
        std::env::var("OXY_GLOBAL_ADMINS").ok().as_deref(),
    );
    if lost.is_empty() {
        return;
    }
    tracing::error!(
        "OXY_APP_ADMINS is set but is NO LONGER READ — rename it to \
         OXY_GLOBAL_ADMINS. These {} address(es) appear ONLY under the old name \
         and are no longer seeded as global admins: {}",
        lost.len(),
        lost.join(", ")
    );
}

/// Emails present in the removed `OXY_APP_ADMINS` and absent from
/// `OXY_GLOBAL_ADMINS` — i.e. exactly what the removal costs this deployment.
/// Split out from the env read so the rule is testable without touching
/// process-global state.
fn legacy_only_emails(legacy: Option<&str>, current: Option<&str>) -> Vec<String> {
    let normalize = |raw: Option<&str>| -> std::collections::BTreeSet<String> {
        raw.unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect()
    };
    let current = normalize(current);
    normalize(legacy)
        .into_iter()
        .filter(|email| !current.contains(email))
        .collect()
}

/// Reads `OXY_GLOBAL_ADMINS` (comma-separated emails) once at startup and
/// inserts any missing rows into `app_admins` with `granted_by = NULL`.
/// Idempotent — re-running is harmless. After the seed, OXY_OWNER users can
/// add/remove admins through the UI; the env var becomes a bootstrap-only
/// convenience, never a permanent allow-list.
///
/// The pre-rename spelling `OXY_APP_ADMINS` is **no longer read**. A
/// deployment still setting only that one would otherwise seed nobody and
/// discover it as "the admin UI is empty", so its presence is called out at
/// startup — see [`warn_on_removed_legacy_admins_env`].
pub async fn bootstrap_app_admins_from_env(db: &DatabaseConnection) -> Result<(), DbErr> {
    warn_on_removed_legacy_admins_env();
    let Ok(raw) = std::env::var("OXY_GLOBAL_ADMINS") else {
        return Ok(());
    };
    let emails: Vec<String> = raw
        .split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    if emails.is_empty() {
        return Ok(());
    }

    let existing: Vec<String> = AppAdmins::find()
        .filter(app_admins::Column::Email.is_in(emails.clone()))
        .all(db)
        .await?
        .into_iter()
        .map(|m| m.email)
        .collect();

    let to_insert: Vec<_> = emails
        .into_iter()
        .filter(|e| !existing.contains(e))
        .map(|email| app_admins::ActiveModel {
            id: sea_orm::ActiveValue::Set(Uuid::new_v4()),
            email: sea_orm::ActiveValue::Set(email),
            granted_by: sea_orm::ActiveValue::Set(None),
            created_at: sea_orm::ActiveValue::NotSet,
            // The env allow-list predates roles and has always meant "full staff", so
            // it seeds Global Admins. A narrower role is a deliberate act performed
            // through the grant API, not something an env var can express — keeping
            // `OXY_GLOBAL_ADMINS` the blunt instrument it already is.
            role: sea_orm::ActiveValue::Set(
                oxy_authz::PlatformRole::GlobalAdmin.as_str().to_string(),
            ),
            scope_all: sea_orm::ActiveValue::Set(true),
            // Seeded, never edited — `updated_at` equals the creation default, which
            // reads correctly as "unchanged since it was granted".
            updated_at: sea_orm::ActiveValue::NotSet,
        })
        .collect();

    if to_insert.is_empty() {
        return Ok(());
    }

    let count = to_insert.len();
    AppAdmins::insert_many(to_insert).exec(db).await?;
    oxy_server_authz::globals::invalidate_admin_cache();
    tracing::info!(
        count,
        "bootstrap_app_admins: seeded {count} global admin(s) from env"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_cache_round_trip() {
        let user = Uuid::new_v4();
        let app = Uuid::new_v4();
        set_cached_access(user, app, true);
        assert_eq!(cached_access(user, app), Some(true));
        invalidate_access_cache();
        assert_eq!(cached_access(user, app), None);
    }

    // What the OXY_APP_ADMINS removal actually costs a given deployment. The
    // rule is keyed on lost addresses rather than on whether the var is set,
    // because the case that loses admins silently is the half-migrated one
    // where BOTH names are set with different contents.

    #[test]
    fn nothing_is_lost_when_the_old_name_is_absent() {
        assert!(legacy_only_emails(None, Some("staff@oxy.tech")).is_empty());
        assert!(legacy_only_emails(Some(""), Some("staff@oxy.tech")).is_empty());
    }

    #[test]
    fn a_fully_migrated_deployment_stays_quiet() {
        // Same people under both names, modulo case and spacing: the operator
        // renamed it and left the old line behind. Nagging every boot would
        // train them to ignore the message that matters.
        assert!(
            legacy_only_emails(
                Some("Staff@oxy.tech, ops@oxy.tech"),
                Some("staff@oxy.tech,ops@oxy.tech"),
            )
            .is_empty()
        );
    }

    #[test]
    fn a_half_migrated_deployment_names_the_addresses_it_drops() {
        // The blind spot in a set-vs-unset rule: both are set, so "already
        // migrated" looks true, but ops@ is seeded by nobody.
        assert_eq!(
            legacy_only_emails(Some("staff@oxy.tech,ops@oxy.tech"), Some("staff@oxy.tech")),
            vec!["ops@oxy.tech".to_string()]
        );
    }

    #[test]
    fn the_new_name_missing_entirely_loses_everyone() {
        assert_eq!(
            legacy_only_emails(Some("staff@oxy.tech,ops@oxy.tech"), None),
            vec!["ops@oxy.tech".to_string(), "staff@oxy.tech".to_string()]
        );
    }
}

#[cfg(test)]
mod user_cache_key_tests {
    use super::user_cache_key;
    use oxy_auth::types::Identity;
    use uuid::Uuid;

    fn session(user_id: Uuid, email: &str) -> Identity {
        Identity {
            user_id: Some(user_id),
            email: email.into(),
            name: None,
            picture: None,
        }
    }

    #[test]
    fn two_workers_with_no_address_do_not_share_a_slot() {
        // The bug: both of these keyed to "" and the second kiosk got the first
        // worker's identity for a minute.
        let maria = session(Uuid::from_u128(1), "");
        let sam = session(Uuid::from_u128(2), "");
        assert_ne!(user_cache_key(&maria), user_cache_key(&sam));
        assert_eq!(user_cache_key(&maria), Uuid::from_u128(1).to_string());
    }

    #[test]
    fn a_session_is_keyed_by_who_it_names_even_with_an_address() {
        // The id is the stronger key for everyone: an address can be re-cased,
        // an id cannot collide.
        let a = session(Uuid::from_u128(3), "Nia@Example.com");
        let b = session(Uuid::from_u128(3), "nia@example.com");
        assert_eq!(user_cache_key(&a), user_cache_key(&b));
    }

    #[test]
    fn a_provider_identity_keys_by_its_lowercased_address() {
        let google = Identity {
            user_id: None,
            email: "Nia@Example.com".into(),
            name: None,
            picture: None,
        };
        assert_eq!(user_cache_key(&google), "nia@example.com");
    }
}
