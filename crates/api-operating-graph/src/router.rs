//! Where this crate's routes mount, and which pod may serve them.
//!
//! Both halves are merged at the protected-tree root by `oxy-server` through
//! `SurfaceSeams::api` — the same place `build_global_routes` held them when
//! they lived in `oxy-app`, so they inherit the standard auth stack from
//! `apply_middleware` rather than reproducing it.

use axum::Router;
use axum::middleware::from_fn;
use axum::routing::{delete, get, patch, put};
use oxy_app_core::AppState;
use oxy_app_core::guards::org_context::org_middleware;
use oxy_app_core::guards::subscription_guard::subscription_guard_middleware;
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

use crate::{assignments, locations, positions, work};

const FLEET: RouteRole = RouteRole::FleetOk;

/// Every operating-graph route: work items at the root, the registry under
/// `/orgs/{org_id}`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(work_routes())
        .nest("/orgs/{org_id}", org_routes())
}

/// Every route [`routes`] mounts, and which pod may serve it: all FleetOk,
/// stated by hand because `RoleRouter::route_fleet`, whose type said so, does
/// not reach across the crate line. Absolute paths: `oxy-server` merges this
/// crate at the protected-tree root, so there is no prefix to join.
pub fn route_roles() -> &'static [RouteRoleDecl] {
    &[
        RouteRoleDecl {
            method: "*",
            path: "/work",
            role: FLEET,
        },
        RouteRoleDecl {
            method: "*",
            path: "/work/{id}",
            role: FLEET,
        },
        RouteRoleDecl {
            method: "*",
            path: "/orgs/{org_id}/locations",
            role: FLEET,
        },
        RouteRoleDecl {
            method: "*",
            path: "/orgs/{org_id}/locations/{id}",
            role: FLEET,
        },
        RouteRoleDecl {
            method: "*",
            path: "/orgs/{org_id}/locations/{id}/external-ids/{system}",
            role: FLEET,
        },
        RouteRoleDecl {
            method: "*",
            path: "/orgs/{org_id}/roles",
            role: FLEET,
        },
        RouteRoleDecl {
            method: "*",
            path: "/orgs/{org_id}/roles/{id}",
            role: FLEET,
        },
        RouteRoleDecl {
            method: "*",
            path: "/orgs/{org_id}/assignments",
            role: FLEET,
        },
        RouteRoleDecl {
            method: "*",
            path: "/orgs/{org_id}/assignments/{id}",
            role: FLEET,
        },
    ]
}

/// The assignment graph's work items. NOT nested under `/orgs/{org_id}`, and
/// that is load-bearing: `org_middleware` resolves an org membership, and a
/// frontline worker holds none by design. Nesting these would lock out exactly
/// the people the graph exists to route work to.
///
/// Authorization is the query filter — you see what you are assigned,
/// supervise, or hold the addressed role for. See [`work`].
fn work_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/work",
            get(work::handlers::list).post(work::handlers::create),
        )
        .route("/work/{id}", patch(work::handlers::update))
}

/// Locations, the positions vocabulary and assignments. Under `/orgs/{org_id}`
/// so the `OrgAdmin` extractor can see the org it is guarding — a body-carried
/// org is invisible to a path-resolved guard.
///
/// Carries `org_middleware` + `subscription_guard`, mirroring oxy-app's
/// `build_org_routes` (where these used to live). `org_middleware` must run
/// BEFORE the guard, which reads the `OrgContext` it inserts; axum applies the
/// last-declared `.layer` outermost, so this order yields
/// `org_middleware → subscription_guard → handler`.
fn org_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/locations",
            get(locations::list_locations).post(work::handlers::create_location),
        )
        .route("/locations/{id}", patch(locations::patch_location))
        .route(
            "/locations/{id}/external-ids/{system}",
            put(locations::put_external_id).delete(locations::delete_external_id),
        )
        .route(
            "/roles",
            get(work::handlers::list_roles).post(work::handlers::create_role),
        )
        .route(
            "/roles/{id}",
            patch(positions::patch_role).delete(positions::delete_role_handler),
        )
        .route(
            "/assignments",
            get(assignments::list).post(assignments::create),
        )
        .route("/assignments/{id}", delete(assignments::delete))
        .layer(from_fn(subscription_guard_middleware))
        .layer(from_fn(org_middleware))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// axum 0.8 checks for route conflicts eagerly as a router is built, so
    /// constructing it is enough to catch a duplicate path within this surface.
    #[test]
    fn routes_build_without_conflict() {
        let _ = routes();
    }

    /// Reproduce the merge point against a stand-in of oxy-app's protected
    /// root, which nests its own `/orgs/{org_id}` tree. A collision then
    /// surfaces here, where axum panics eagerly on `.merge`, instead of as a
    /// boot panic.
    #[test]
    fn merges_into_the_protected_root_without_conflict() {
        async fn h() {}
        let _root: Router<AppState> = Router::new()
            .route("/chat/channels", get(h))
            .nest("/orgs/{org_id}", Router::new().route("/members", get(h)))
            .merge(routes());
    }

    /// Nine paths, every one FleetOk.
    #[test]
    fn every_route_is_declared_fleet_ok() {
        let decls = route_roles();
        assert_eq!(decls.len(), 9);
        assert!(decls.iter().all(|d| d.role == RouteRole::FleetOk));
    }
}
