//! The workspaces surface, out of `oxy-app` (Wave C of the core routes,
//! `internal-docs/domain-boundaries.md`): a workspace as its org and its
//! members see it.
//!
//! - [`workspaces`]: an org's workspace list, rename and delete, a
//!   workspace's metadata (`/meta`), and the details document the curated
//!   OpenAPI document describes.
//! - [`workspace_members`]: a workspace's members and their per-workspace role
//!   overrides.
//! - [`workspace_oxy_access`]: the org's switch that locks Oxy staff out of a
//!   workspace.
//! - [`apps`]: the world-model app integrations `config.yml` declares, listed
//!   for Workspace Settings.
//!
//! [`workspace_routes`] rides the workspace seam, merged INSIDE the
//! `/api/{workspace_id}` nest, so it inherits the session auth stack, the
//! workspace subscription guard and `workspace_middleware`, where
//! `build_workspace_routes` mounted these, in both modes. [`routes`] rides the
//! api seam at the protected root, under `/orgs/{org_id}` with `org_middleware`
//! and the subscription guard re-applied as `build_org_routes` wrapped them:
//! cloud only, like the global tree it left. Every route is FleetOk, method
//! `*`, as `route_fleet` declared them: Postgres, the compiled config and the
//! read-only workspace manager; the working copy only where the serving
//! process owns one (local mode's details, a delete's files).
//!
//! [`openapi`] carries the two operations the curated OpenAPI document has
//! always listed (`GET /workspaces/{workspace_id}`, `GET
//! /orgs/{org_id}/workspaces`), through `SurfaceSeams::openapi`.

// The same headroom every surface crate takes (see
// `crates/server/src/recursion_limit_guard.rs`): an async handler's future can
// exceed rustc's default query depth in the `--release` image build only.
#![recursion_limit = "256"]

pub mod apps;
mod authz;
pub mod workspace_members;
pub mod workspace_oxy_access;
pub mod workspaces;

use axum::Router;
use axum::middleware::from_fn;
use axum::routing::{delete, get, patch, put};
use oxy_app::surface::{org_middleware, subscription_guard_middleware};
use oxy_app_core::AppState;
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

/// The api seam's share: the org's workspace list, delete and rename, under
/// `/orgs/{org_id}`, merged at the protected root (`/api`) where
/// `build_global_routes` nested `build_org_routes`.
///
/// Carries `org_middleware` + `subscription_guard`, as `build_org_routes`
/// wrapped them. `org_middleware` must run BEFORE the guard, which reads the
/// `OrgContext` it inserts; axum applies the last-declared `.layer` outermost,
/// so this order yields `org_middleware → subscription_guard → handler`.
pub fn routes() -> Router<AppState> {
    Router::new().nest("/orgs/{org_id}", org_routes())
}

fn org_routes() -> Router<AppState> {
    org_handlers()
        .layer(from_fn(subscription_guard_middleware))
        .layer(from_fn(org_middleware))
}

/// The org routes without their two layers, so the mount test can reach the
/// method router without a database (`org_middleware` reads one).
///
/// The delete removes the row, its schedules and its Airhouse tenant record:
/// Postgres only. No comment sits directly above a `.route(`: the route catalog
/// would read one as the route's note, and these routes had none. The list is
/// named through `workspaces::handlers` so the catalog finds its doc rather
/// than the staff compile console's `admin::compiles::workspaces::list_workspaces`.
fn org_handlers() -> Router<AppState> {
    Router::new()
        .route("/workspaces", get(workspaces::handlers::list_workspaces))
        .route("/workspaces/{id}", delete(workspaces::delete_workspace))
        .route(
            "/workspaces/{id}/rename",
            patch(workspaces::rename_workspace),
        )
}

const fn fleet(path: &'static str) -> RouteRoleDecl {
    RouteRoleDecl {
        method: "*",
        path,
        role: RouteRole::FleetOk,
    }
}

/// Pod placement for [`routes`], absolute (merged at the protected root, so
/// there is no prefix to join): the three rows `oxy-app`'s `route_fleet`
/// declared in `build_org_routes`, method `*`, FleetOk.
pub fn route_roles() -> &'static [RouteRoleDecl] {
    ROUTE_ROLES
}

/// A `const`, not a literal in the function: `fleet` is a `const fn`, and a
/// slice of its calls is promoted to `'static` only through a constant.
const ROUTE_ROLES: &[RouteRoleDecl] = &[
    fleet("/orgs/{org_id}/workspaces"),
    fleet("/orgs/{org_id}/workspaces/{id}"),
    fleet("/orgs/{org_id}/workspaces/{id}/rename"),
];

/// The workspace seam's share: merged inside `/api/{workspace_id}`, in both
/// modes. `/app-integrations` is nested, as `oxy-app` nested it, so the prefix
/// answers exactly what it did (the list is its `/`).
///
/// `/meta` is the Postgres half of the retired `/details`, which served both
/// halves and was the last route needing the ide-down degrade mechanism. On
/// `/oxy-access`, POST locks Oxy staff OUT and DELETE lifts the
/// lockdown (the default is access-granted); only a REAL org officer may do
/// either. No comment sits directly above a `.route(`: the route catalog would
/// read one as the route's note, and these routes had none.
pub fn workspace_routes() -> Router<AppState> {
    Router::new()
        .route("/meta", get(workspaces::get_workspace_meta))
        .route("/members", get(workspace_members::list_workspace_members))
        .route(
            "/members/{user_id}",
            put(workspace_members::set_workspace_role_override)
                .delete(workspace_members::remove_workspace_role_override),
        )
        .route(
            "/oxy-access",
            get(workspace_oxy_access::get_oxy_access)
                .post(workspace_oxy_access::lock_oxy_access)
                .delete(workspace_oxy_access::unlock_oxy_access),
        )
        .nest("/app-integrations", app_integration_routes())
}

fn app_integration_routes() -> Router<AppState> {
    Router::new().route("/", get(apps::list_apps))
}

/// Pod placement for [`workspace_routes`], relative to the workspace tree: the
/// five rows `oxy-app`'s `route_fleet` declared, method `*`, FleetOk. The
/// nested `/` declares the prefix itself (`/app-integrations`), as
/// `RoleRouter::nest` joined it.
pub fn workspace_route_roles() -> &'static [RouteRoleDecl] {
    WORKSPACE_ROUTE_ROLES
}

const WORKSPACE_ROUTE_ROLES: &[RouteRoleDecl] = &[
    fleet("/meta"),
    fleet("/members"),
    fleet("/members/{user_id}"),
    fleet("/oxy-access"),
    fleet("/app-integrations"),
];

/// The operations this surface documents in the served OpenAPI document
/// (`/apidoc/openapi.json`, which `oxyc schema` reads), merged in by the
/// composition root through `SurfaceSeams::openapi`: the two `oxy-app`'s
/// `openapi_router` listed. `GET /workspaces/{workspace_id}` is documented
/// but mounted on no router, as it was before the move.
pub fn openapi() -> utoipa::openapi::OpenApi {
    utoipa_axum::router::OpenApiRouter::<AppState>::new()
        .routes(utoipa_axum::routes!(workspaces::get_workspace))
        .routes(utoipa_axum::routes!(workspaces::list_workspaces))
        .into_openapi()
}

#[cfg(test)]
mod tests;
