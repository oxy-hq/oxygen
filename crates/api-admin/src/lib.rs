//! The staff console's own crate: the cross-context staff machinery that no
//! tenant context owns (`internal-docs/domain-boundaries.md` § The admin
//! split, S1).
//!
//! Started in the admin split's PR 7 with the durable task fleet's operator
//! console ([`internal_jobs`], `/api/admin/internal-jobs/*`), out of
//! `oxy-app`'s `api::admin::internal_jobs*`. The platform audit log
//! ([`audit`], `/api/admin/audit*`), the grant table's routes
//! ([`app_admins`], `/api/admin/app-admins*`) and the assume-role routes
//! ([`assume`], `/api/assume*`) followed on 2026-11-20, and the feature-flag
//! routes ([`feature_flags`]: the staff half at `/api/admin/feature-flags*`
//! and the org member's read at `/api/orgs/{org_id}/feature-flags`) on
//! 2026-11-21. `routing` follows in a later PR, and the console shell
//! (`admin::router`: the door, the capability layers, `extracted_sections`)
//! last, with the router assembly.
//!
//! Only the staff console's machinery goes here: Oxy's own operational state,
//! which no tenant context owns. A section about a tenant context goes to
//! that context's surface, never here by audience: orgs went to
//! `oxy-api-tenancy`, the user directory to `oxy-api-identity`, the compile
//! console to `oxy-api-previews`. Feature flags are Oxy's own product flags,
//! operated by staff, so both halves of that context sit here (S1): the org
//! member's read is the flags' exposure point, not a tenant context of its
//! own.
//!
//! Mounted by the `oxy-server` composition root through three seams:
//! `SurfaceSeams::admin_nests` ([`admin_nests`]), chained ahead of
//! `oxy-api-previews`' compile console so the route-role table keeps the
//! order it had in `oxy-app` (internal jobs, then compiles),
//! `SurfaceSeams::admin` ([`admin_sections`]), the audit log, the grant
//! table and the feature-flag console, merged inside `/api/admin` as they
//! were in `oxy-app`, and `SurfaceSeams::api` ([`routes`]), assume-role and
//! the org member's flag read, merged at the root of the protected tree
//! OUTSIDE `/admin`, as `oxy-app`'s global tree merged them.
//! Every route reads or writes Postgres only (the task queue, the run,
//! workspace, audit, grant, assume-session and feature-flag tables, and the
//! flag cache; the audit anchor reads S3, never a working copy), so all
//! three are FleetOk.

// The same headroom every surface crate takes (see
// `crates/server/src/recursion_limit_guard.rs`).
#![recursion_limit = "256"]

pub mod app_admins;
pub mod assume;
pub mod audit;
pub mod feature_flags;
pub mod internal_jobs;
mod internal_jobs_dead_letter;
mod internal_jobs_failure_groups;
mod internal_jobs_fleet;
mod internal_jobs_reach;
mod internal_jobs_rows;

use axum::Router;
use oxy_app::surface::roles::RouteRole;
use oxy_app::surface::{Action, AdminNest, AdminSection};
use oxy_app_core::AppState;
use oxy_shared::fleet_role::RouteRoleDecl;

/// The routes this crate hands `SurfaceSeams::api`: assume-role (`/assume`,
/// `/assume/current`, `/assume/history`), out of `oxy-app`'s
/// `admin::assume` (admin split, 2026-11-20), and the org member's flag read
/// (`/orgs/{org_id}/feature-flags`, [`feature_flags::org_routes`]), out of
/// `oxy-app`'s `build_org_routes` (2026-11-21).
///
/// **Not a section or a nest, on purpose.** Both would put the routes behind
/// the staff door and the refusal while acting: a partner, who is not staff,
/// could not reach them at all, and staff could start a session and never end
/// it, because acting closes `/admin`. The exit cannot live behind the door it
/// locks. The api seam merges it where `oxy-app`'s `build_global_routes` did:
/// the root of the protected tree, behind the authentication stack alone
/// (and internal auth on the internal port), cloud only. The handlers
/// authorize (`may_act_as`, `PlatformAudit` for the history).
///
/// Assume-role has no declarations: the routes are Postgres rows only, so the
/// FleetOk default is the truth, as it was for the undeclared merge in
/// `oxy-app`.
///
/// The flag read is NOT under the staff door either: any org member reads it,
/// behind `org_middleware` and no subscription guard (the shell reads it on
/// the paywall too), as `build_org_routes` mounted it outside its gated tree.
/// Declared FleetOk in [`route_roles`], as `route_fleet` declared it there.
pub fn routes() -> Router<AppState> {
    assume::router().merge(feature_flags::org_routes())
}

/// Pod placement for [`routes`], absolute (merged at the protected root, so
/// there is no prefix to join): the one row `oxy-app`'s
/// `route_fleet("/feature-flags", ..)` declared inside `/orgs/{org_id}`,
/// method `*`. The flag cache is loaded from Postgres on every replica.
pub fn route_roles() -> &'static [RouteRoleDecl] {
    ROUTE_ROLES
}

const ROUTE_ROLES: &[RouteRoleDecl] = &[RouteRoleDecl {
    method: "*",
    path: "/orgs/{org_id}/feature-flags",
    role: RouteRole::FleetOk,
}];

/// The staff console's sibling trees this crate hands `oxy-app`: today the
/// task fleet's operator console, nested at `/api/admin/internal-jobs` behind
/// the staff door, the refusal while acting and `PlatformOperate`.
///
/// Internal jobs is a nest rather than a section because its routes are flat
/// (no `/internal-jobs/` prefix on each), so the guards answer every path it
/// mounts, a wrong method included (a path the tree does not match is the
/// outer router's 404). Operating the durable task fleet is Oxy's own
/// machinery, not tenant data — `PlatformOperate` — and the handlers narrow by
/// grant scope themselves (`internal_jobs_reach`). `oxy-app` applies the
/// guards; this crate only names the capability.
pub fn admin_nests() -> Vec<AdminNest> {
    vec![AdminNest {
        prefix: "/internal-jobs",
        capability: Action::PlatformOperate,
        routes: internal_jobs::router(),
        role: RouteRole::FleetOk,
        why: "job admin reads the task queue tables",
    }]
}

/// The staff console's sections this crate hands `oxy-app`, merged inside
/// `/api/admin` before the refusal while acting, each behind the capability
/// `admin::router` gave it in-tree: the platform audit log (`/audit`,
/// `/audit/verify/{org_id}`, `/audit/anchor/{org_id}`) behind
/// `PlatformAudit`, the grant table (`/app-admins`, `/app-admins/{id}`)
/// behind `PlatformGrants`, and the feature-flag console (`/feature-flags`,
/// `/feature-flags/evaluate`, `/feature-flags/{key}`) behind
/// `PlatformOperate`: the flags are deployment-wide operational config, beside
/// routing and the task fleet.
///
/// A section, not a nest, because that is how it was mounted: a
/// `route_layer` on a router merged inside `admin::router`, ahead of the
/// refusal while acting, which is exactly what `extracted_sections` builds.
/// A nest would add its own `{prefix}` and `{prefix}/{*rest}` role rows and
/// catalog seed under a second staff door; the section adds nothing. It
/// declares nothing: the console's `/{*rest}` FleetOk wildcard places it,
/// as it did. `oxy-app` applies the guard; this crate only names the
/// capability. The audit handlers narrow by grant scope themselves
/// (`staff::scope`).
///
/// **The grant table is capability-gated, not owner-only.** It was owner-only
/// for one release, on the reasoning that "a capability that could edit the
/// grant table would let its holder widen their own grant, and the ceiling
/// would mean nothing". That objection is real and is answered by bounding the
/// write rather than withholding the capability: `may_delegate` admits only a
/// grant strictly weaker than the writer's own, so the one row a holder can
/// never touch is their own, and only the owner can mint a peer (see
/// `oxy_server_authz::staff::delegation`). **The capability gate here is a
/// door, not the control**: the handlers carry the row-level half
/// (`actor_facts` once, then `refuse(may_delegate(..))` per row), exactly as
/// scope works, and `app_scope_boundary` pins both halves. Billing stays the
/// console's one owner-only gate (`Ring::GlobalOwnerOnly`, `oxy-app`'s
/// `admin::router`). Moved here from `admin::router`'s doc with the routes.
pub fn admin_sections() -> Vec<AdminSection> {
    vec![
        AdminSection {
            capability: Action::PlatformAudit,
            routes: audit::router(),
            decls: Vec::new(),
        },
        AdminSection {
            capability: Action::PlatformGrants,
            routes: app_admins::router(),
            decls: Vec::new(),
        },
        AdminSection {
            capability: Action::PlatformOperate,
            routes: feature_flags::router(),
            decls: Vec::new(),
        },
    ]
}

#[cfg(test)]
mod tests;
