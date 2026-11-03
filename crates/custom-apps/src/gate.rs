//! The decision half of the custom-app data-plane gate: who the caller is, which
//! workspace the request is for, and whether that caller may read its data.
//!
//! `oxy-app`'s `custom_apps_gates` runs the gate chain for every custom-app
//! endpoint (`POST /api/projects/:id/query`, `/semantic-query`,
//! `/agents/.../asks`, `/automations/.../runs`): cookie / API-key auth, Origin
//! allowlist, user lookup, DB connection, workspace lookup, org-membership
//! check. The steps that need HTTP types (`HeaderMap`, the authenticator, the
//! response) stay there. The steps that only read the database and decide —
//! the user lookup, the workspace row, the shipped access check, the ring's
//! term — are here, and report a failure as a [`GateError`] (a status, a
//! message and an optional code) that `oxy-app` turns into the response.
//!
//! ## Cost, and what is — and is not — remembered
//!
//! The chain is memoised where memoising cannot over-admit, and nowhere else:
//!
//! - the **user** for 60 s (the cache the bundle path reads);
//! - the **ring's term** — `allows(facts, WorkspaceDataAccess, workspace)` —
//!   per `(user_id, project_id, org_id)` for 60 s, dropped by every
//!   `invalidate_access_cache` hook. A term is remembered only when the
//!   facts loaded; the facts-unknown arm defers to the shipped verdict for
//!   that one request and remembers nothing.
//!
//! Everything that scopes or decides the request is read **fresh on every
//! call**: the workspace row (its `org_id` is the tenant boundary the whole
//! decision is keyed by — a moved or deleted workspace is judged anew on
//! every replica at once, which no replica-local cache could promise), and
//! the shipped gate — membership, frontline grant, staff reach, lockdown,
//! partner reach. The decision is `shipped && term`. The term can only
//! subtract (`oxy_authz::enforce`'s contract), so what the cache can do to a
//! viewer is over-refuse for up to a minute after a grant is added; it can
//! never keep admitting one after a membership, a frontline grant, a partner
//! ceiling, a lockdown or the workspace's org changed. Staff reach is read
//! through `oxy_server_authz::globals`, which its own writers invalidate on
//! the writing replica and which ages out in 60 s elsewhere — the bound
//! every guard in the product already has. A token that narrows its bearer
//! reads and writes no term.
//!
//! What that costs on a warm replica, before the handler's own work: an org
//! member pays two primary-key-shaped queries (the workspace row,
//! `is_org_member`); a non-member pays the row and the false arm (frontline
//! standing, lockdown, partner reach). Before this the chain cost 7-8
//! queries for a member — the user, the workspace, the membership walk and
//! the ~4-query principal-facts load — on every data call.
//!
//! The staging pin is per request by design.

use entity::prelude::Workspaces;
use oxy_auth::authenticator::Authenticator;
use oxy_auth::built_in::BuiltInAuthenticator;
use oxy_auth::types::{AuthenticatedUser, Identity};
use oxy_auth::user::UserService;
use oxy_platform::db::DbFailure;
use oxy_server_authz as authz;
use oxy_shared::errors::OxyError;
use sea_orm::{DatabaseConnection, EntityTrait};
use tracing::error;
use uuid::Uuid;

use crate::auth::{is_org_member, is_oxy_locked_down, user_cache_key};
use crate::cache::{cached_model_term, cached_user, remember_model_term, set_cached_user};
use crate::staging_pin::PinNotReady;

/// Resolved context produced by the gate chain. The handler unpacks whichever
/// fields it needs; the struct is the minimum set of state every custom-app
/// endpoint needs.
///
/// `oxy-app` adds `build_project_context` to it (a method that returns
/// `OxyProjectContext`, which lives above this crate).
pub struct CustomAppContext {
    /// Authenticated user (id, email, name, status). Derived from the
    /// subject of the project-scoped app token during the gate chain.
    pub user: AuthenticatedUser,
    /// Project UUID (= workspace id) the request is scoped to.
    pub project_id: Uuid,
    /// Workspace row resolved from `project_id`.
    pub workspace: entity::workspaces::Model,
    /// Organization that owns the workspace. Guaranteed non-`None`
    /// here — the gate rejects workspaces without an `org_id`.
    pub org_id: Uuid,
    /// Live DB connection — reused by handlers that need additional
    /// queries (e.g. agent lookup) so we don't pay for a second
    /// `establish_connection()` round-trip per request.
    pub db: DatabaseConnection,
    /// Set only on a STAGING request (the app's staging host, opened by a viewer
    /// who may see staging) whose calling app's staging build pins a semantic
    /// revision — see
    /// `custom_apps_staging_pin::request`. `None` for every live request.
    pub staging_pin: Option<Uuid>,
}

impl CustomAppContext {
    /// Result-cache partition for this request: `""` on live, the pin on a
    /// pinned staging request, so a branch's answer never serves live users
    /// and live's never answers the staging preview.
    pub fn cache_scope(&self) -> String {
        match self.staging_pin {
            Some(pin) => format!("staging-pin:{pin}"),
            None => String::new(),
        }
    }
}

/// Why a decision step refused or failed, as plain values: the HTTP status, the
/// message and the optional machine-readable code the response body carries.
/// `oxy-app` builds the response from them, so every status and message is
/// stated here once and pinned by `tests`.
///
/// Every 500 raised off a `DbErr` carries a `code` from [`DbFailure`], because
/// the two failures behind one need opposite responses and the message cannot
/// tell them apart. A local box spent an afternoon on
/// `{"message":"workspace lookup failed"}` on ~40% of requests: `localhost`
/// resolved to `::1` first and the IPv6 loopback reset every new connection
/// mid-handshake, while warm ones kept serving. That is `database_unreachable`
/// on the wire — a different thing to go looking at than
/// `database_query_failed`, which is ours.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateError {
    /// The credential names no user. Never cached.
    UserNotFound,
    /// The user lookup failed.
    UserLookupFailed { code: &'static str },
    /// The workspace does not exist, or an API token has no grant on it — the
    /// two answer alike so a token cannot probe which workspaces exist.
    ProjectNotFound,
    /// The workspace row lookup failed.
    WorkspaceLookupFailed { code: &'static str },
    /// The workspace row has no `org_id`.
    NoOwningOrganization,
    /// The org-membership query failed.
    MembershipCheckFailed { code: &'static str },
    /// No credential, or one the authenticator refuses.
    AuthenticationRequired,
    /// The request's `Origin` is not on the allow-list.
    OriginNotAllowed,
    /// The pool could not be built.
    DatabaseUnavailable { code: &'static str },
    /// The caller may not read the owning organization's data.
    NotAMember,
    /// The request's environment follows a semantic branch whose head has no
    /// ready compile. Retryable (`503` + `Retry-After`): the plane's home is
    /// not ready, and the request is refused rather than read from another
    /// model. The body names the commit and the way out
    /// ([`PinNotReady::fix`]).
    SemanticBranchNotReady(PinNotReady),
}

impl GateError {
    /// The HTTP status code of the response.
    pub fn status(&self) -> u16 {
        match self {
            GateError::UserNotFound | GateError::AuthenticationRequired => 401,
            GateError::ProjectNotFound => 404,
            GateError::NoOwningOrganization
            | GateError::OriginNotAllowed
            | GateError::NotAMember => 403,
            GateError::UserLookupFailed { .. }
            | GateError::WorkspaceLookupFailed { .. }
            | GateError::MembershipCheckFailed { .. }
            | GateError::DatabaseUnavailable { .. } => 500,
            GateError::SemanticBranchNotReady(_) => 503,
        }
    }

    /// The `message` field of the response body.
    pub fn message(&self) -> &'static str {
        match self {
            GateError::UserNotFound => "user not found",
            GateError::UserLookupFailed { .. } => "user lookup failed",
            GateError::ProjectNotFound => "project not found",
            GateError::WorkspaceLookupFailed { .. } => "workspace lookup failed",
            GateError::NoOwningOrganization => "workspace has no owning organization",
            GateError::MembershipCheckFailed { .. } => "org membership check failed",
            GateError::AuthenticationRequired => "authentication required",
            GateError::OriginNotAllowed => "origin not allowed",
            GateError::DatabaseUnavailable { .. } => "database unavailable",
            GateError::NotAMember => "not a member of the owning organization",
            GateError::SemanticBranchNotReady(_) => "the semantic branch is not ready",
        }
    }

    /// The refusal behind a `503 semantic_branch_not_ready`, whose body also
    /// names the commit and the fix.
    pub fn pin_not_ready(&self) -> Option<&PinNotReady> {
        match self {
            GateError::SemanticBranchNotReady(refusal) => Some(refusal),
            _ => None,
        }
    }

    /// The `code` field of the response body, when there is one.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            GateError::UserLookupFailed { code }
            | GateError::WorkspaceLookupFailed { code }
            | GateError::MembershipCheckFailed { code }
            | GateError::DatabaseUnavailable { code } => Some(*code),
            GateError::SemanticBranchNotReady(_) => Some("semantic_branch_not_ready"),
            GateError::UserNotFound
            | GateError::ProjectNotFound
            | GateError::NoOwningOrganization
            | GateError::AuthenticationRequired
            | GateError::OriginNotAllowed
            | GateError::NotAMember => None,
        }
    }
}

/// The whole gate chain, for every custom-app data-plane endpoint: returns the
/// resolved context, or the refusal for `oxy-app` (or a surface crate) to turn
/// into a response. Auth runs first so an unauthenticated caller cannot probe
/// a route through an extractor error.
///
/// Context-based access: when the app is served by oxy (in-workspace or admin
/// preview) the request carries the session cookie; external/dev callers send a
/// bearer token. Either way the caller must be able to access the project (org
/// member or oxy app-admin).
///
/// Warm, an org member pays two queries here — the workspace row and
/// `is_org_member`, both asked fresh on every request by design: the user comes
/// from the 60 s cache, the ring's term from the `(user_id, project_id, org_id)`
/// cache, and on a production request the staging pin is answered from the
/// headers alone.
pub async fn check_custom_app_gates(
    headers: &http::HeaderMap,
    project_id: Uuid,
) -> Result<CustomAppContext, GateError> {
    // ── 0. Sentry, barrier 2: this request is a custom app's ──────────
    //
    // Entering this gate IS the identity. Every route behind it is a
    // custom-app data-plane route and the web app calls none of them, so
    // the hub can be tagged here without guessing at a URL — which the URL
    // rules cannot do for this plane anyway: a bundle fetches same-origin
    // with an empty `apiBaseUrl`, and its origin is `app.oxygen-hq.com`
    // (canonical `/customer-apps/<org>/<app>/` URL) or an org subdomain,
    // neither of which carries a custom-app host label or a
    // `/customer-apps/**` path. See `sentry_surface::mark_custom_app_surface`.
    //
    // FIRST, before authentication, so the tag covers everything captured
    // for the rest of this request: the handler's own `error!` on a
    // warehouse failure (whose target is `…::projects::query`, which
    // barrier 1 does not drop), `OxyError::capture_to_sentry`, and a panic.
    oxy_telemetry::sentry_filter::mark_custom_app_surface();

    // ── 1. Authenticate ───────────────────────────────────────────────
    // Session cookie (served by oxy) or a bearer token (external/dev).
    //
    // With the credential: an API token's grants and standing flags decide the
    // access check below exactly as they do on `/api`.
    let (identity, credential) =
        match BuiltInAuthenticator::new(oxy_auth::token::SandboxAgent::Refuse)
            .authenticate_with_credential(headers)
            .await
        {
            Ok(authenticated) => authenticated,
            Err(_) => return Err(GateError::AuthenticationRequired),
        };

    // ── 2. Origin allowlist ───────────────────────────────────────────
    if !oxy_app_core::origin::is_allowed_origin(headers) {
        tracing::warn!("custom-app gate: request rejected — origin not in allowlist");
        return Err(GateError::OriginNotAllowed);
    }

    // ── 3. User lookup, through the 60 s user cache ───────────────────
    // Attached AFTER the cache: the cached row is the user, never the
    // credential one request happened to arrive with.
    let user = gate_user(&identity).await?.with_credential(credential);
    let caller = authz::Caller::from_user(&user);

    // ── 4. DB connection ──────────────────────────────────────────────
    // No `DbErr` to classify — failing to build the pool at all is unreachable
    // by construction, so state the same code the lookups behind it reach by
    // inspection.
    let db = oxy::database::client::establish_connection()
        .await
        .map_err(|e| {
            error!(
                code = DbFailure::Unreachable.code(),
                error = %e,
                "DB connection failed"
            );
            GateError::DatabaseUnavailable {
                code: DbFailure::Unreachable.code(),
            }
        })?;

    // ── 5. Workspace + access: the row and the gate fresh, the term cached ─
    let verdict = gate_verdict(&db, &caller, project_id).await?;
    if !verdict.allowed {
        return Err(GateError::NotAMember);
    }

    // ── 6. Staging pin, per request ───────────────────────────────────
    // The staging build's semantic pin, for a request on the app's staging
    // host, decided by the same reach rule that serves that viewer the
    // staging bundle. After the access decision, so it can only narrow WHICH
    // revision an already authorized request reads, never whether it may
    // read. Never cached: it is a property of this request's host and
    // headers, and a production request answers it from them alone.
    // A branch with no ready compile is refused, not read from another model.
    let staging_pin = crate::staging_pin::request::staging_pin_for_data_request(
        &db, headers, &caller, project_id,
    )
    .await
    .map_err(GateError::SemanticBranchNotReady)?;

    Ok(CustomAppContext {
        user,
        project_id,
        workspace: verdict.workspace,
        org_id: verdict.org_id,
        db,
        staging_pin,
    })
}

/// Step 3: the user the credential names, from the cache the bundle path
/// keeps (`auth::user_cache_key` — the id, or the address only
/// for a provider identity naming nobody yet). A miss is loaded and cached;
/// a credential naming nobody is `401`, never cached.
pub async fn gate_user(identity: &Identity) -> Result<AuthenticatedUser, GateError> {
    let cache_key = user_cache_key(identity);
    if let Some(user) = cached_user(&cache_key) {
        return Ok(user);
    }
    match UserService::find_user_by_identity(identity).await {
        Ok(Some(user)) => {
            set_cached_user(cache_key, user.clone());
            Ok(user)
        }
        Ok(None) => Err(GateError::UserNotFound),
        Err(e) => {
            let code = user_lookup_failure_code(&e);
            error!(code, error = %e, "user lookup failed");
            Err(GateError::UserLookupFailed { code })
        }
    }
}

/// Step 5: the workspace row, then the decision — both the row and the
/// shipped gate FRESH on every request, the ring's term from the `(user_id,
/// project_id, org_id)` cache. See the module doc for what that bounds.
///
/// Failures (a workspace that does not exist, a lookup error) are answered,
/// never remembered. A token that narrows its bearer is decided fresh in
/// full: the term is a SESSION's, and the cache key does not carry the grant.
pub async fn gate_verdict(
    db: &DatabaseConnection,
    caller: &authz::Caller,
    project_id: Uuid,
) -> Result<GateVerdict, GateError> {
    let workspace = load_workspace(db, project_id).await?;
    // The access boundary: the caller must be a member of the project's
    // org, OR an oxy app-admin (so staff can use the admin-panel preview,
    // which is served by oxy and authenticated by the same cookie). Same-
    // origin bundles can issue XHRs at any project, so this membership
    // check — not the Origin allowlist — is what gates cross-project data.
    let Some(org_id) = workspace.org_id else {
        return Err(GateError::NoOwningOrganization);
    };
    // An API token with no grant on this workspace answers exactly as an unknown
    // workspace does, so it cannot probe which workspaces exist. A session and
    // a legacy key have no grants to be outside of.
    if caller.workspace_ceiling(org_id, project_id).is_none() {
        return Err(GateError::ProjectNotFound);
    }
    let shipped = shipped_workspace_access(db, caller, org_id, project_id).await?;
    // ENFORCE the unified ring alongside the shipped check: the decision is
    // `shipped && term`, so the model can only ever tighten it — a mis-modeled
    // ring can't hand a partner another tenant's data. Cannot be sampled: an
    // authorization decision has to be deterministic.
    let allowed = shipped && model_term(db, caller, org_id, project_id, shipped).await;
    Ok(GateVerdict {
        workspace,
        org_id,
        allowed,
    })
}

/// What step 5 decided: the row the handlers run against, and whether this
/// request may.
pub struct GateVerdict {
    pub workspace: entity::workspaces::Model,
    pub org_id: Uuid,
    pub allowed: bool,
}

/// The workspace row, read fresh: one primary-key lookup, and the tenant
/// boundary of everything that follows. A missing workspace is `404`, a
/// lookup error `500`.
async fn load_workspace(
    db: &DatabaseConnection,
    project_id: Uuid,
) -> Result<entity::workspaces::Model, GateError> {
    match Workspaces::find_by_id(project_id).one(db).await {
        Ok(Some(ws)) => Ok(ws),
        Ok(None) => Err(GateError::ProjectNotFound),
        Err(e) => {
            let failure = DbFailure::classify(&e);
            error!(
                code = failure.code(),
                error = %e,
                workspace = %project_id,
                "workspace lookup failed"
            );
            Err(GateError::WorkspaceLookupFailed {
                code: failure.code(),
            })
        }
    }
}

/// The shipped gate, decided fresh on every request.
///
/// Access: a real org member, an Oxy operator whose grant reaches this org
/// with `develop_apps`, a partner operator whose ceiling grants `develop_apps`
/// over the org's managing partner — or a frontline worker holding a grant on
/// an app published from this workspace.
///
/// NOTE — this is the DATA PLANE. `develop_apps` is the read-the-app's-data
/// capability, deliberately DISTINCT from `manage_apps` (app lifecycle:
/// publish / unpublish). A partner that can toggle an app's visibility cannot
/// read its data unless its ceiling ALSO grants `develop_apps`; the split is
/// what keeps "manage the app" from silently meaning "read the client's data".
/// Granularity is deliberately org-wide (a developing partner works across the
/// client's workspaces) and the grant is the ceiling itself (staff-set), so the
/// data plane is not separately consent-gated the way publishing is. See
/// `partner_authz::partner_grants_app_access` (called below).
///
/// The staff term is the SAME call `user_can_access_app` makes — reach named
/// by capability and org, then the workspace's lockdown switch. It used to be
/// a bare `is_staff()`, which admitted every platform role to every tenant's
/// data plane here while the app-keyed gate refused them next door, and
/// ignored the lockdown an org had explicitly set. An unreadable lockdown
/// counts as locked: that only ever costs staff a request.
///
/// The frontline term sits inside the non-member arm because a worker is not
/// an org member and never will be — `enroll_worker` writes `users.email =
/// NULL` precisely to keep them out of every email-keyed path — so for the
/// ordinary members who are 99% of callers it costs nothing. It decides
/// standing AND grant by itself: the ring's term runs only when the principal
/// facts load, and on a loader error the caller keeps this verdict as-is, so a
/// term that admitted every active worker and trusted the ring to narrow it
/// would have admitted every worker to every app in the org for the duration
/// of a database blip.
async fn shipped_workspace_access(
    db: &DatabaseConnection,
    caller: &authz::Caller,
    org_id: Uuid,
    project_id: Uuid,
) -> Result<bool, GateError> {
    match is_org_member(db, caller.user_id, org_id).await {
        Ok(true) => Ok(true),
        Ok(false) => Ok(
            frontline_worker_with_app_grant(db, org_id, caller.user_id, project_id).await
                || (authz::globals::platform_reaches(db, caller, authz::Cap::DevelopApps, org_id)
                    .await
                    && !is_oxy_locked_down(db, project_id).await.unwrap_or(true))
                || authz::partner_authz::partner_grants_app_access(db, caller, org_id).await,
        ),
        Err(e) => {
            let failure = DbFailure::classify(&e);
            error!(
                code = failure.code(),
                error = %e,
                workspace = %project_id,
                "org membership check failed"
            );
            Err(GateError::MembershipCheckFailed {
                code: failure.code(),
            })
        }
    }
}

/// The ring's term, `allows(facts, WorkspaceDataAccess, workspace)`, from the
/// `(user_id, project_id, org_id)` cache or computed — and remembered only when the
/// facts loaded.
///
/// The resource is the WORKSPACE, stated as one. This gate is keyed on
/// `project_id`, and every route behind it serves workspace data shared by
/// every app published from that workspace — there is no app id on the wire.
/// It used to hand the ring `Resource::app(project_id, …)`, a workspace wearing
/// an app's type: `app_restricted` could never be true, and the frontline grant
/// (`facts.app_memberships`, app ids) could never match, so a granted worker
/// got `true && false` on every healthy request and this gate grew an
/// exemption that skipped the ring for them. `Ring::WorkspaceData` reads the
/// fact the loader now derives for a worker — the workspaces their app grants
/// were published from — so the conjunction holds and the model decides,
/// here as everywhere. `tests/platform/frontline_app_grant.rs` holds the two
/// sides to each other against real rows.
///
/// The load is scoped (no workspace-override query, which this ring never
/// reads). Unknown facts (a lookup errored) defer to the shipped verdict
/// rather than denying — the conjunction only subtracts, so deferring can't
/// open a hole, and a blip must not 403 every legitimate app user — and that
/// deferral is this request's alone: nothing is cached on that arm.
async fn model_term(
    db: &DatabaseConnection,
    caller: &authz::Caller,
    org_id: Uuid,
    project_id: Uuid,
    shipped: bool,
) -> bool {
    let cacheable = !caller.reach().is_some_and(oxy_authz::TokenReach::narrows);
    if cacheable && let Some(term) = cached_model_term(caller.user_id, project_id, org_id) {
        return term;
    }
    let action = authz::Action::WorkspaceDataAccess;
    let resource = authz::Resource::workspace(project_id, org_id);
    let term = authz::loader::load_principal_facts_scoped(db, caller, false)
        .await
        .map(|facts| {
            // `enforce` is the warn-on-disagreement; its verdict is
            // `shipped && allows`, which the caller restates with the term.
            authz::enforce("gate.custom_app", &facts, action, &resource, shipped);
            oxy_authz::allows(&facts, action, &resource)
        });
    if cacheable {
        remember_model_term(caller.user_id, project_id, org_id, term);
    }
    term.unwrap_or(true)
}

/// The code a failed user lookup ships with.
///
/// `oxy-auth` classified the error while it still held the `DbErr`; by here it
/// is a string wearing a variant, so read the variant. `Database` is
/// `establish_connection` failing to build the pool at all — the lookup's first
/// call — so it gets the same verdict step 4 states for that failure.
fn user_lookup_failure_code(e: &OxyError) -> &'static str {
    match e {
        OxyError::DatabaseUnreachable(_) | OxyError::Database(_) => DbFailure::Unreachable.code(),
        _ => DbFailure::Query.code(),
    }
}

/// An active frontline worker holding an explicit grant on an app in this
/// workspace.
///
/// Both halves, in one place, because the gate's frontline term must be able to
/// deny by itself. It cannot rely on `Ring::WorkspaceData` narrowing it
/// afterwards: `enforce` runs only when the principal facts load, and on a
/// loader error the caller keeps this verdict as-is.
///
/// It is also the shipped check that ring is differenced against — the loader
/// derives `frontline_workspace_grants` from the same join, and
/// `the_model_and_both_gates_agree_about_a_worker` in
/// `tests/platform/frontline_app_grant.rs` asserts the two never disagree.
///
/// Scoped to the WORKSPACE the gate was called for, not to the org. A grant on
/// some other workspace's app is not access to this one — that is the whole
/// point of a per-app grant, and joining through `apps.project_id` is what keeps
/// it true. Without the join this would read "has a grant anywhere in the org",
/// which is the org-wide reach a frontline worker must not have.
///
/// Errors are `false`. A worker who cannot be checked is not admitted.
///
/// `pub` so `tests/platform/frontline_app_grant.rs` can drive it against a real
/// database. It is the whole decision — the gate above it is one guard line —
/// and two rounds of review found defects in exactly this logic that no unit
/// test could have reached.
pub async fn frontline_worker_with_app_grant(
    db: &DatabaseConnection,
    org_id: Uuid,
    user_id: Uuid,
    project_id: Uuid,
) -> bool {
    // Standing first: it is one indexed lookup and most callers are not
    // frontline, so this short-circuits the join for every ordinary request.
    match oxy_auth::frontline::is_active_frontline(db, org_id, user_id).await {
        Ok(true) => {}
        _ => return false,
    }
    // Reads `app_members` directly rather than going through
    // `auth::has_app_grant`, which is documented as the one place
    // the two grant kinds are unioned. Deliberate, and it costs nothing today:
    // `has_app_grant` takes an APP id and this gate has only a workspace, and
    // the other grant kind — a team grant — cannot reach a frontline worker,
    // because `add_team_member` rejects anyone who is not an org member and a
    // worker never is one. So the union is empty for this principal by
    // construction. If team membership is ever opened to workers, this is the
    // second place that has to learn about it.
    use entity::{app_members, apps};
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect, RelationTrait};

    app_members::Entity::find()
        .filter(app_members::Column::UserId.eq(user_id))
        .join(
            sea_orm::JoinType::InnerJoin,
            app_members::Relation::Apps.def(),
        )
        .filter(apps::Column::ProjectId.eq(project_id))
        // Published apps only — the same line `user_can_access_app` draws for
        // every customer. A grant on a draft must not open the workspace's
        // data plane while the app's own shell still refuses to load.
        .filter(apps::Column::PublishedAt.is_not_null())
        .one(db)
        .await
        .map(|row| row.is_some())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `establish_connection` failing to build the pool arrives as `Database`,
    /// and `find_user_by_identity` calls it first — that is a connect failure,
    /// the same verdict step 4 states, not a bad query.
    #[test]
    fn a_pool_that_cannot_be_built_reads_as_unreachable() {
        assert_eq!(
            user_lookup_failure_code(&OxyError::Database("pool timed out".into())),
            DbFailure::Unreachable.code()
        );
        assert_eq!(
            user_lookup_failure_code(&OxyError::DatabaseUnreachable("reset".into())),
            DbFailure::Unreachable.code()
        );
        assert_eq!(
            user_lookup_failure_code(&OxyError::DBError("syntax error".into())),
            DbFailure::Query.code()
        );
    }

    /// Status, message and code are the wire contract the handlers' tests and
    /// the SDK read; this pins each refusal the decision half can raise.
    #[test]
    fn each_refusal_states_its_status_message_and_code() {
        let unreachable = DbFailure::Unreachable.code();
        let cases = [
            (GateError::UserNotFound, 401, "user not found", None),
            (
                GateError::UserLookupFailed { code: unreachable },
                500,
                "user lookup failed",
                Some(unreachable),
            ),
            (GateError::ProjectNotFound, 404, "project not found", None),
            (
                GateError::WorkspaceLookupFailed { code: unreachable },
                500,
                "workspace lookup failed",
                Some(unreachable),
            ),
            (
                GateError::NoOwningOrganization,
                403,
                "workspace has no owning organization",
                None,
            ),
            (
                GateError::MembershipCheckFailed { code: unreachable },
                500,
                "org membership check failed",
                Some(unreachable),
            ),
            (
                GateError::AuthenticationRequired,
                401,
                "authentication required",
                None,
            ),
            (GateError::OriginNotAllowed, 403, "origin not allowed", None),
            (
                GateError::DatabaseUnavailable { code: unreachable },
                500,
                "database unavailable",
                Some(unreachable),
            ),
            (
                GateError::NotAMember,
                403,
                "not a member of the owning organization",
                None,
            ),
        ];
        for (error, status, message, code) in cases {
            assert_eq!(error.status(), status, "{error:?}");
            assert_eq!(error.message(), message, "{error:?}");
            assert_eq!(error.code(), code, "{error:?}");
        }
    }
}
