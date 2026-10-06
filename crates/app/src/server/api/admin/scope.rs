//! The one place a staff handler asks "does my grant reach this org?".
//!
//! `platform_cap_guard` cannot answer it. It decides on `Resource::platform()`, which has
//! a nil org — so a **bounded** `global_admin` passes every capability gate on this
//! console, and narrowing is left to the handler. That split is deliberate (capabilities
//! gate verbs, scope filters rows) and it has now been got wrong three times, each time
//! in a file that had no reason to know the rule existed:
//!
//! * the custom-app registry — fixed with `app_scope_guard` plus three handler checks;
//! * org membership — `add_to_org` could grant Owner in any tenant;
//! * org and workspace administration — `DELETE /admin/orgs/{any}` ran unfenced, which
//!   is the reach this whole change opens by describing ("every Global Admin could delete
//!   any org") and outranks the other two: a wrongly-added member can be removed, a
//!   deleted org cannot be un-deleted.
//!
//! Two identical copies of this fence already existed, in `users_admin` and
//! `apps::handlers`, differing only in imports. A third copy was the wrong answer, so
//! this is the shared one.

use axum::http::StatusCode;
use oxy_auth::types::AuthenticatedUser;
use oxy_authz::Scope;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use crate::server::authz::{Caller, globals, loader};

/// The caller's platform scope — the **one** read of a grant's scope on this console.
///
/// Every fence below and [`list_scope`] take it from here, so a row named by id and a
/// row in a listing cannot be judged by two readings of one grant. Scope is a *fact*:
/// the loader reads the grant (`load_platform_facts`) and the model states where it
/// reaches (`PrincipalFacts::platform_scope` — `Scope::All` for the Global Owner). No
/// function here inspects a grant row.
///
/// `Ok(None)` is no standing at all. **`Err(500)` is a grant that could not be read**,
/// and every caller propagates it with `?`: unknown never reads as unbounded, and never
/// as "no standing" either, which a fence lets through.
async fn caller_scope(
    db: &DatabaseConnection,
    actor: &AuthenticatedUser,
    doing: &'static str,
) -> Result<Option<Scope>, StatusCode> {
    // Credential-aware: a token that carries no platform standing reads as none.
    match loader::load_platform_facts(db, &Caller::from_user(actor)).await {
        Some(facts) => Ok(facts.platform_scope().cloned()),
        None => {
            tracing::error!(target: "authz", "platform grant unreadable {doing} — refusing");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Refuse when the caller's platform grant does not reach `org_id`.
///
/// **404, not 403** — an operator with no reach into an org must not learn it exists by
/// being told "forbidden". Consistent with every other out-of-scope answer on this
/// branch.
///
/// **500 on an unreadable grant** (from [`caller_scope`]). These are write paths;
/// treating "unknown" as "unbounded" is how one transient `DbErr` hands out tenant
/// Owner or drops a tenant. The lenient read-path behaviour lives in
/// `apps::handlers::scope_org_filter` and is deliberately not shared with this.
///
/// Callers pass their own connection: this module opens none, so a handler that already
/// has a handle does not pay for a second. `platform_cap_guard` opens its own because it
/// runs before any handler exists.
pub async fn deny_out_of_scope(
    db: &DatabaseConnection,
    actor: &AuthenticatedUser,
    org_id: Uuid,
) -> Result<(), StatusCode> {
    // The Global Owner is root and holds no grant row — their standing is the env
    // allow-list, which no outage takes away (the rule `platform_cap_guard` states), so
    // they pass before anything is read.
    if globals::is_global_owner(&Caller::from_user(actor)) {
        return Ok(());
    }
    match caller_scope(db, actor, "on a scoped admin WRITE").await? {
        Some(Scope::All) => Ok(()),
        Some(Scope::Orgs(orgs)) if orgs.contains(&org_id) => Ok(()),
        Some(Scope::Orgs(_)) => Err(StatusCode::NOT_FOUND),
        // Unreachable in practice rather than merely permitted: `platform_cap_guard`'s
        // oracle is table-only, and `allows` needs a standing, so a caller with no grant
        // row cannot be here at all unless they are the owner who returned above. Kept
        // as the defensive default; if this arm ever executes, the guard above it changed
        // and that is the thing to look at. ([`list_scope`] answers the same caller with
        // an empty listing — the two differ, and `admin_staff_scope::fences` pins both.)
        None => Ok(()),
    }
}

/// The same fence for a resource whose org is **nullable** — a workspace.
///
/// `if let Some(org) = ws.org_id { deny(..) }` was the obvious spelling and the wrong
/// one: the `None` arm is not a check that passes, it is no check at all, so an
/// org-less workspace was deletable by any bounded grant. It also defeated the boundary
/// test, which asserts the fence is *called* — and it was, conditionally.
///
/// `None` refuses for a bounded grant, because a null org is by definition not in
/// `Scope::Orgs(..)`. Unbounded grants and the owner still pass, which is the same
/// answer they get everywhere else. This is the direction the whole branch settled on:
/// an org that cannot be established means refuse, exactly as an unreadable grant does.
pub async fn deny_out_of_scope_opt(
    db: &DatabaseConnection,
    actor: &AuthenticatedUser,
    org_id: Option<Uuid>,
) -> Result<(), StatusCode> {
    match org_id {
        Some(org_id) => deny_out_of_scope(db, actor, org_id).await,
        None => {
            if globals::is_global_owner(&Caller::from_user(actor)) {
                return Ok(());
            }
            match caller_scope(db, actor, "fencing an org-less resource").await? {
                Some(Scope::Orgs(_)) => Err(StatusCode::NOT_FOUND),
                Some(Scope::All) | None => Ok(()),
            }
        }
    }
}

/// The org ids a staff **listing** must be narrowed to — `None` when the caller's grant
/// is unbounded (an all-orgs grant, or the Global Owner).
///
/// The listing twin of [`deny_out_of_scope`], reading the grant through the same
/// [`caller_scope`], so a handler never inspects a grant row itself — it takes this
/// answer and puts it **in its query**, before `LIMIT`/`OFFSET`, so a page is never
/// short and a `COUNT` never includes a row the caller cannot see.
///
/// Three rules, said once here:
///
/// * **A row with no org is platform-level.** `org_id = ANY(..)` never matches `NULL`,
///   so a bounded grant does not see an org-less row (a system job, a platform audit
///   event). Only an unbounded grant and the Global Owner do — the same answer
///   [`deny_out_of_scope_opt`] gives a single org-less resource.
/// * **500 on an unreadable grant.** These listings carry tenant content — audit rows,
///   task payloads, run errors — so "unknown" must not read as "unbounded". That is the
///   opposite of `apps::handlers::scope_org_filter`, which lists a registry the caller
///   could already enumerate and prefers showing rows to showing an error.
/// * **No standing reaches nothing.** `platform_cap_guard` makes that unreachable over
///   HTTP; if the guard above a handler ever changes, the listing comes back empty
///   rather than whole.
pub async fn list_scope(
    db: &DatabaseConnection,
    actor: &AuthenticatedUser,
) -> Result<Option<Vec<Uuid>>, StatusCode> {
    Ok(
        match caller_scope(db, actor, "narrowing a staff listing").await? {
            Some(Scope::All) => None,
            Some(Scope::Orgs(orgs)) => Some(orgs),
            None => Some(Vec::new()),
        },
    )
}

/// [`deny_out_of_scope_opt`] for a resource named by **workspace**: resolves the
/// workspace's org and fences on that.
///
/// A missing workspace answers the same `404` an out-of-scope one does, so the route
/// cannot be used to probe the workspace directory. An org-less workspace refuses for a
/// bounded grant, exactly as the `_opt` fence says.
pub async fn deny_out_of_scope_for_workspace(
    db: &DatabaseConnection,
    actor: &AuthenticatedUser,
    workspace_id: Uuid,
) -> Result<(), StatusCode> {
    use sea_orm::EntityTrait;
    let workspace = entity::workspaces::Entity::find_by_id(workspace_id)
        .one(db)
        .await
        .map_err(|e| {
            tracing::error!(target: "authz", error = %e, "workspace unreadable on a scoped admin route");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;
    deny_out_of_scope_opt(db, actor, workspace.org_id).await
}

/// Refuse a **platform-level** operation — one that acts on every tenant at once and so
/// belongs to no org — for a bounded grant. Unbounded grants and the Global Owner pass.
///
/// Spelled as a function rather than `deny_out_of_scope_opt(.., None)` at each call
/// site so the reason is on the page: a fleet-wide reaper run or retention sweep is not
/// "an org-less row", it is an action whose reach is the whole deployment.
pub async fn deny_out_of_scope_platform(
    db: &DatabaseConnection,
    actor: &AuthenticatedUser,
) -> Result<(), StatusCode> {
    deny_out_of_scope_opt(db, actor, None).await
}

/// ` AND <column> = ANY($n)` for a bounded grant, with the org set bound as one
/// `uuid[]`; the empty string for an unbounded one.
///
/// One bound value however many orgs a grant names, so the statement shape is constant
/// and an empty scope correctly matches nothing. `column` is a compile-time literal at
/// every call site (`"w.org_id"`), never request input. The placeholder is numbered
/// from `values.len()`, so push every fixed parameter **before** calling this.
pub(crate) fn org_scope_clause(
    column: &str,
    scope: Option<&[Uuid]>,
    values: &mut Vec<sea_orm::Value>,
) -> String {
    match scope {
        None => String::new(),
        Some(orgs) => {
            values.push(orgs.to_vec().into());
            format!(" AND {column} = ANY(${})", values.len())
        }
    }
}
