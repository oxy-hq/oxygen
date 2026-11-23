//! The tenant-facing observability console's HTTP surface, out of `oxy-app`:
//! the IDE's Traces, Metrics and Execution Analytics pages. The
//! store and its queries are `oxy-observability`'s (the ClickHouse product
//! store, `OXY_OBSERVABILITY_BACKEND` / `OXY_CLICKHOUSE_*`); this crate is the
//! handlers that scope a read to the workspace and shape the answer.
//!
//! [`workspace_routes`] rides the workspace seam, merged INSIDE the
//! `/api/{workspace_id}` nest, so it inherits the session auth stack and
//! `workspace_middleware`, as the three `nest_all` mounts in `oxy-app`'s
//! `build_workspace_routes` did:
//!
//! - `/traces` ([`traces`]): the trace list and one trace's spans.
//! - `/metrics` ([`metrics`]): metric usage analytics, the list, one metric.
//! - `/execution-analytics` ([`execution_analytics`]): verified vs generated
//!   executions, latency, cost.
//!
//! [`admin`] is the crate's other half, and a different audience: the staff
//! console's **Admin → Observability**, per org — what an org costs us, and
//! how its custom apps perform. It rides the `/api/admin` seam as an
//! [`AdminSection`](oxy_app_core::seams::AdminSection) under `PlatformOperate`,
//! not the workspace seam, because its reads are cross-tenant and its gate is
//! platform standing rather than a workspace session.
//!
//! Every route is FleetOk. The old mounts' reasons said "the runtime tables",
//! which reads as Postgres; the reads go to the observability store, which is
//! ClickHouse, and every replica reaches it the same way (or none does: with no
//! store configured every node answers the same 500). No handler touches the
//! working copy, which `oxy-app`'s `undeclared_mounts_stay_diskless` checks by
//! scanning this crate.
//!
//! Nothing here depends on `oxy-app`. The tenant handlers take `AppState`
//! (`oxy-app-core`). [`admin`] takes the `AdminSection` seam and the console's
//! connect-on-demand DB handle from `oxy-app-core` too, and the staff-scope
//! fence from `oxy-server-authz`, never deciding a bounded grant's reach by
//! hand. That dependency on `oxy-app` (2026-11-16) went away in decompose R1
//! (2026-11-23). Nothing depends back on this crate, so a surface edit still
//! does not rebuild oxy-app.

// The same headroom every surface crate takes (see
// `crates/server/src/recursion_limit_guard.rs`): an async handler's future can
// exceed rustc's default query depth in the `--release` image build only.
#![recursion_limit = "256"]

pub mod admin;
pub mod execution_analytics;
pub mod metrics;
pub mod traces;

use axum::Router;
use oxy_app_core::AppState;
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

/// The staff-console section this crate contributes under `/api/admin`, for
/// `oxy-server` to merge — named at the crate root beside [`workspace_routes`]
/// so a composition reads both of this surface's seams in one place.
pub use admin::{admin_sections, route_roles as admin_route_roles};

/// The admin section's routes, relative to `/admin`. A function rather than a
/// `pub use … as admin_routes`: the route catalog's seed (`admin_routes` in
/// this module) resolves functions only, so the re-export left every route of
/// this section out of `oxyc routes` with only a build warning to say so.
pub fn admin_routes() -> Router<AppState> {
    admin::routes()
}

/// The workspace seam's share: merged inside `/api/{workspace_id}`. Nested,
/// as `oxy-app` nested them, so each prefix answers exactly what it did.
pub fn workspace_routes() -> Router<AppState> {
    Router::new()
        .nest("/traces", traces::traces_routes())
        .nest("/metrics", metrics::metrics_routes())
        .nest(
            "/execution-analytics",
            execution_analytics::execution_analytics_routes(),
        )
}

const fn fleet(path: &'static str) -> RouteRoleDecl {
    RouteRoleDecl {
        method: "*",
        path,
        role: RouteRole::FleetOk,
    }
}

/// Pod placement for [`workspace_routes`], relative to the workspace tree.
/// The six rows `oxy-app`'s `nest_all` declared for these three nests: the
/// prefix and its `{*rest}`, method `"*"`, each FleetOk.
pub fn workspace_route_roles() -> &'static [RouteRoleDecl] {
    WORKSPACE_ROUTE_ROLES
}

/// A `const`, not a literal in the function: `fleet` is a `const fn`, and a
/// slice of its calls is promoted to `'static` only through a constant.
const WORKSPACE_ROUTE_ROLES: &[RouteRoleDecl] = &[
    fleet("/traces/{*rest}"),
    fleet("/traces"),
    fleet("/metrics/{*rest}"),
    fleet("/metrics"),
    fleet("/execution-analytics/{*rest}"),
    fleet("/execution-analytics"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use tower::ServiceExt;

    /// Every concrete route the three nests mount, with path parameters filled.
    const MOUNTED: &[&str] = &[
        "/traces",
        "/traces/0af7651916cd43dd8448eb211c80319c",
        "/metrics/analytics",
        "/metrics/list",
        "/metrics/revenue",
        "/execution-analytics/summary",
        "/execution-analytics/time-series",
        "/execution-analytics/agents",
        "/execution-analytics/executions",
        "/execution-analytics/percentiles",
        "/execution-analytics/histogram",
        "/execution-analytics/cost",
    ];

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

    /// The routes came from `oxy-app`, where `nest_all` wrote these rows. Here
    /// the role is a declaration, so hold it: each row is DECLARED FleetOk
    /// exactly once — not merely unclassified, which `classify` also answers
    /// FleetOk and is what a dropped row would look like.
    #[test]
    fn every_route_is_declared_fleet_ok_exactly_once() {
        let declared = workspace_route_roles();
        assert_eq!(declared.len(), 6, "three nests, a prefix and a rest each");
        for decl in declared {
            let same = declared
                .iter()
                .filter(|d| d.method == decl.method && d.path == decl.path)
                .count();
            assert_eq!(
                same, 1,
                "{} {}: declared {same} times",
                decl.method, decl.path
            );
            assert_eq!(decl.role, RouteRole::FleetOk, "{}", decl.path);
            assert_eq!(decl.method, "*", "{}", decl.path);
        }
    }

    /// The declarations hold against the role classifier the way the served
    /// router uses them: prefixed by the workspace tree, then matched against a
    /// live URI. Every mounted route must match a declaration, not the default.
    #[test]
    fn every_mounted_route_classifies_fleet_ok() {
        use oxy_app_core::pods::role_manifest::declared_role_in;
        let decls: Vec<_> = workspace_route_roles()
            .iter()
            .map(|d| (d.method, format!("/api/{{workspace_id}}{}", d.path), d.role))
            .collect();
        for path in MOUNTED {
            let uri = format!("/api/d9830be4-c6a4{path}");
            assert_eq!(
                declared_role_in(&decls, "GET", &uri),
                Some(RouteRole::FleetOk),
                "GET {uri} must match a FleetOk declaration"
            );
        }
    }

    /// A declaration with no route behind it (or a route moved without its
    /// declaration) leaves the two lists disagreeing. TRACE, which no route
    /// here takes, answers 405 from a mounted path and 404 from an unmounted
    /// one, without running a handler.
    #[tokio::test]
    async fn every_route_is_mounted() {
        let router = workspace_routes().with_state(oxy_app_core::app_state::bare_app_state());
        for uri in MOUNTED {
            assert_eq!(
                trace(&router, uri).await,
                StatusCode::METHOD_NOT_ALLOWED,
                "{uri}: not mounted"
            );
        }
        // The bare `/metrics` and `/execution-analytics` prefixes are not
        // routes (neither nest has a `/`); `fleet-routes.tsv` records them as
        // 404 live. Held so the nesting keeps answering exactly what it did.
        for uri in ["/metrics", "/execution-analytics", "/nothing-mounts-this"] {
            assert_eq!(
                trace(&router, uri).await,
                StatusCode::NOT_FOUND,
                "{uri} must stay unmounted"
            );
        }
    }
}
