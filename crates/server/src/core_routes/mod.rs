//! The core routes that left `oxy-app`'s `server/api/` for the binary (the
//! core-routes plan, Wave A, 2026-11-16): the health probes
//! ([`healthcheck`]) and route discovery ([`catalog`]). Neither belongs to a
//! bounded context, so neither went sideways into an `api-*` surface: they
//! describe the process and the deployment, which is what the composition root
//! is.
//!
//! They ride the seams like any surface, so `oxy-app` mounts them where it
//! mounted them itself:
//!
//! - [`public_routes`] on `SurfaceSeams::public`: beside `build_public_routes`,
//!   outside the auth stack, in both serve modes and on the internal port;
//! - [`catalog_routes`] on `SurfaceSeams::catalog`: at the root of the
//!   protected tree, behind the auth stack, in both serve modes and on the
//!   internal port;
//! - [`openapi`] on `SurfaceSeams::openapi`: the four probes' operations in the
//!   served document.
//!
//! What they read about the build (`GIT_HASH`, the image tag) comes from
//! [`crate::build_identity`], which this crate's build script stamps (up from
//! `oxy-app` in decompose BID, 2026-11-25); `oxy migrate` reports it too.

pub mod catalog;
pub mod healthcheck;

use axum::Router;
use axum::routing::get;
use oxy_app_core::AppState;
use oxy_route_catalog::RouteCatalog;
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

const fn fleet(path: &'static str) -> RouteRoleDecl {
    RouteRoleDecl {
        method: "*",
        path,
        role: RouteRole::FleetOk,
    }
}

/// The health probes, relative to `/api`. Unauthenticated by design (an
/// uptime monitor and kubelet carry no session), so they ride the public seam.
pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(healthcheck::health_check))
        .route("/ready", get(healthcheck::readiness_check))
        .route("/live", get(healthcheck::liveness_check))
        .route("/version", get(healthcheck::version_info))
}

/// Pod placement for [`public_routes`]: the four rows `oxy-app`'s
/// `route_fleet` declared, method `*`, FleetOk. A probe must answer on every
/// replica, never only on the Factory.
pub fn public_route_roles() -> &'static [RouteRoleDecl] {
    const ROLES: &[RouteRoleDecl] = &[
        fleet("/health"),
        fleet("/ready"),
        fleet("/live"),
        fleet("/version"),
    ];
    ROLES
}

/// Route discovery for the `oxyc` CLI: `GET /api/_catalog`.
///
/// ITS OWN BUILDER, and that is not stylistic. `crates/route-catalog/build_route_catalog.rs`
/// walks a fixed list of SEED functions to generate the route table, and this
/// is one of them. A route written inline anywhere else is invisible to the
/// very catalog it serves unless that builder is seeded, so `/api/_catalog`
/// would be the one endpoint missing from the endpoint list. `catalog_tests`'
/// `the_catalog_lists_itself` fails if it ever stops being seeded.
///
/// `catalog` is the generated table, bound into the handler here. Mounted in
/// BOTH the cloud and local protected routers (and the internal one), because
/// local mode omits the global tree: `SurfaceSeams::catalog` is merged at the
/// protected root in every mode.
///
/// FleetOk ([`catalog_route_roles`]): the handler reads a table compiled into
/// the binary (no workspace working copy, no `.git`, no state dir), so any
/// replica may answer it, and pinning it to the singleton would make the CLI's
/// discovery depend on the one instance that can be down.
pub fn catalog_routes(catalog: RouteCatalog) -> Router<AppState> {
    Router::new().route(
        "/_catalog",
        get(move |query| catalog::get_catalog(catalog, query)),
    )
}

/// Pod placement for [`catalog_routes`]: the row `oxy-app`'s `route_fleet`
/// declared, method `*`, FleetOk.
pub fn catalog_route_roles() -> &'static [RouteRoleDecl] {
    const ROLES: &[RouteRoleDecl] = &[fleet("/_catalog")];
    ROLES
}

/// The probes' OpenAPI operations (tag `Health`), merged into the served
/// document through `SurfaceSeams::openapi`, as `oxy-app`'s curated
/// `openapi_router` listed them.
pub fn openapi() -> utoipa::openapi::OpenApi {
    utoipa_axum::router::OpenApiRouter::<AppState>::new()
        .routes(utoipa_axum::routes!(healthcheck::health_check))
        .routes(utoipa_axum::routes!(healthcheck::readiness_check))
        .routes(utoipa_axum::routes!(healthcheck::liveness_check))
        .routes(utoipa_axum::routes!(healthcheck::version_info))
        .into_openapi()
}
