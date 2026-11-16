//! The staff console's Explorer (`/api/admin/explorer/{threads,runs}`): a
//! cross-tenant, read-only search over threads and runs, so an operator can go
//! from "this conversation looks broken" to the tenant that owns it.
//!
//! Moved out of `oxy-app`'s `admin::explorer` in the admin split. It is the
//! Conversation context's staff face (S1: "admin" is an audience, not a
//! context), so it rides `SurfaceSeams::admin` from this crate rather than from
//! a staff-console crate. `oxy-app` merges the section inside `/api/admin`,
//! under the staff guard and the acting block, and applies the capability it
//! names — the same three layers `admin::router` gave the route in-tree.

pub mod explorer;

use oxy_app::surface::{Action, AdminSection};
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

/// The capability that admits a caller: `PlatformExplorer` (`view_tenants`),
/// the one `admin::router` layered on the Explorer in `oxy-app`.
pub const CAPABILITY: Action = Action::PlatformExplorer;

/// This surface's staff-console section, for `oxy-server` to merge.
pub fn admin_sections() -> Vec<AdminSection> {
    vec![AdminSection {
        capability: CAPABILITY,
        routes: crate::admin_routes(),
        decls: route_roles().to_vec(),
    }]
}

/// Pod placement, relative to `/admin`: both searches read Postgres only
/// (`threads`, `agentic_runs` and the tenant joins), never a working copy, so
/// `FleetOk`, as `oxy-app`'s `/admin/{*rest}` wildcard resolved them.
pub fn route_roles() -> &'static [RouteRoleDecl] {
    ROUTE_ROLES
}

const ROUTE_ROLES: &[RouteRoleDecl] = &[
    RouteRoleDecl {
        method: "*",
        path: "/explorer/threads",
        role: RouteRole::FleetOk,
    },
    RouteRoleDecl {
        method: "*",
        path: "/explorer/runs",
        role: RouteRole::FleetOk,
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use tower::ServiceExt;

    const MOUNTED: &[&str] = &["/explorer/threads", "/explorer/runs"];

    async fn trace(router: &Router, uri: &str) -> StatusCode {
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::TRACE)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }

    /// TRACE answers 405 from a mounted path and 404 from an unmounted one,
    /// without running a handler.
    #[tokio::test]
    async fn every_route_is_mounted() {
        let router = crate::admin_routes().with_state(oxy_app::surface::bare_app_state());
        for uri in MOUNTED {
            assert_eq!(
                trace(&router, uri).await,
                StatusCode::METHOD_NOT_ALLOWED,
                "{uri}: not mounted"
            );
        }
        for uri in ["/explorer", "/explorer/agents"] {
            assert_eq!(
                trace(&router, uri).await,
                StatusCode::NOT_FOUND,
                "{uri} must stay unmounted"
            );
        }
    }

    /// Each row is declared `FleetOk` exactly once, and matches the live URI
    /// once prefixed the way the served router prefixes it.
    #[test]
    fn every_route_is_declared_fleet_ok_exactly_once() {
        use oxy_app::surface::roles::declared_role_in;
        let declared = route_roles();
        assert_eq!(declared.len(), MOUNTED.len(), "one row per mounted route");
        let decls: Vec<_> = declared
            .iter()
            .map(|d| (d.method, format!("/api/admin{}", d.path), d.role))
            .collect();
        for path in MOUNTED {
            assert_eq!(
                declared_role_in(&decls, "GET", &format!("/api/admin{path}")),
                Some(RouteRole::FleetOk),
                "GET /api/admin{path}"
            );
        }
    }

    /// The section names `PlatformExplorer`, the gate the Explorer had in
    /// `oxy-app`. Widening it to `PlatformOperate` or `PlatformApps` would
    /// hand every tenant's conversations to a role that holds neither
    /// `view_tenants` nor any reason to read them.
    #[test]
    fn the_section_is_gated_on_platform_explorer() {
        let sections = admin_sections();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].capability, Action::PlatformExplorer);
    }
}
