//! The metric-monitoring HTTP surface: the IDE's metric tree and the anomaly
//! inbox, out of `oxy-app` (Pending 8 in `internal-docs/domain-boundaries.md`).
//! The computation is `oxy-metric-monitoring`'s; this crate is the handlers.
//!
//! Two trees mount it, as they mounted these handlers when they lived in
//! `oxy-app`'s `build_workspace_routes` and `build_external_workspace_routes`:
//!
//! - [`workspace_routes`] rides the workspace seam, merged INSIDE the
//!   `/api/{workspace_id}` nest, so it inherits the session auth stack and
//!   `workspace_middleware`: the ten `/semantic/metric-tree*` routes,
//!   `/semantic/monitors` and the `/semantic/anomalies` inbox.
//! - [`external_routes`] rides the external seam, merged into the curated
//!   `/external/api/{workspace_id}` tree before its API-key-only auth: the same
//!   anomaly inbox, so the TypeScript SDK's `client.anomalies.*` works against
//!   `/external/api` as well as `/api`.
//!
//! Every route is FleetOk: the metric tree resolves its scan root through the
//! compile boundary first (`resolve_scan`), working copy second, and runs on
//! the workspace's warehouses; the inbox reads and writes Postgres rows and
//! reads `.monitor.yml` from the compiled revision. The routes left
//! `route_fleet`, whose type refused a working-copy extractor, so here the role
//! is a declaration ([`workspace_route_roles`], held by the tests below) and
//! `oxy-app`'s `undeclared_mounts_stay_diskless` scans this crate for disk
//! access.
//!
//! The anomaly handlers extract `Extension<Arc<AgenticState>>` (for its
//! database handle). `oxy-app` layers that extension over the workspace and
//! external seams' routes (`SurfaceSeams`), because the agentic state is built
//! inside `api_router`, after the composition root hands the seams in.
//!
//! What stays below, in `oxy-workspace-runtime` (`oxy-app`'s `agentic_wiring`
//! until 2026-11-22): the executor (`metric_tree_runner`), the executor-facing
//! core the customer-app twins share (`metric_tree_exec`), both imported from
//! there directly.
//!
//! Insights delivery is split by layer: the enqueue a scan calls is
//! `oxy_metric_monitoring::notify::enqueue_if_due`, and its task's executor is
//! [`anomaly_notify`], handed to the global-run driver through [`tasks`]
//! (`SurfaceSeams::tasks`).

// The same headroom every surface crate takes (see
// `crates/server/src/recursion_limit_guard.rs`): an async handler's future can
// exceed rustc's default query depth in the `--release` image build only.
#![recursion_limit = "256"]

pub mod anomaly_notify;
pub mod metric_anomalies;
pub mod metric_tree;
pub mod metric_tree_probe;
pub mod metric_tree_projection;

use axum::Router;
use axum::routing::{get, post};
use oxy_app_core::AppState;
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

pub use anomaly_notify::tasks;

/// The workspace seam's share: merged inside `/api/{workspace_id}`.
pub fn workspace_routes() -> Router<AppState> {
    Router::new()
        // Metric tree — structure + pure analysis ops over the semantic model.
        // Merged as its own sub-router so the diagnostic probe layers over
        // these ten routes and nothing else.
        .merge(metric_tree_routes())
        // `/semantic/monitors` is merged rather than nested: nesting would also
        // answer on `/semantic/monitors/`, which nothing calls.
        .route("/semantic/monitors", get(metric_anomalies::list_monitors))
        .nest("/semantic/anomalies", anomaly_routes())
}

/// The external seam's share: merged inside `/external/api/{workspace_id}`.
///
/// The anomaly inbox only, used by standalone apps (e.g. world-model-app) that
/// cannot reach the internal `/api` surface. The same builder the workspace
/// tree nests, not a copy of it. `/scan` and `/{anomaly_id}/explain` are
/// long-running (a scan waits up to 55 s before answering `pending: true`; an
/// uncached explain runs a 20-30 s recursive search) but bounded by the same
/// `timeout_middleware` the rest of that tree carries.
pub fn external_routes() -> Router<AppState> {
    Router::new().nest("/semantic/anomalies", anomaly_routes())
}

/// Metric-tree structure + analysis ops, carrying the diagnostic probe.
///
/// A sub-router purely so [`metric_tree_probe::probe`] covers exactly these
/// routes: layering it on the workspace router would time every workspace
/// endpoint, and the question it answers is about these ten. Paths stay
/// absolute (merged, not nested) so each declaration reads as its URI.
///
/// FleetOk on every route: the scan root resolves through the compile boundary
/// first, working copy second, and warehouse execution needs only config +
/// secrets — so a replica answers these. The outage behind that is
/// oxy-hq/oxygen#878 (every call 500'd on the serve fleet when these read
/// `semantics_scan_path()` directly).
fn metric_tree_routes() -> Router<AppState> {
    Router::new()
        .route("/semantic/metric-tree", get(metric_tree::get_metric_tree))
        .route(
            "/semantic/metric-tree/{measure_id}/sensitivity",
            get(metric_tree::get_sensitivity),
        )
        .route(
            "/semantic/metric-tree/predict",
            post(metric_tree::post_predict),
        )
        .route(
            "/semantic/metric-tree/explain",
            post(metric_tree::post_explain),
        )
        .route(
            "/semantic/metric-tree/opportunity",
            post(metric_tree::post_opportunity),
        )
        .route(
            "/semantic/metric-tree/drill",
            post(metric_tree::post_opportunity_drill),
        )
        .route(
            "/semantic/metric-tree/time-dimensions",
            get(metric_tree::get_time_dimensions),
        )
        .route(
            "/semantic/metric-tree/distribution",
            post(metric_tree::post_distribution),
        )
        .route(
            "/semantic/metric-tree/baseline",
            post(metric_tree::post_baseline),
        )
        .route(
            "/semantic/metric-tree/projection",
            post(metric_tree_projection::post_projection),
        )
        .layer(axum::middleware::from_fn(metric_tree_probe::probe))
}

/// The anomaly inbox, nested at `/semantic/anomalies` on both trees.
fn anomaly_routes() -> Router<AppState> {
    Router::new()
        .route("/", get(metric_anomalies::list_anomalies))
        // Reads `.monitor.yml` and the semantic model from the compiled
        // revision, as the scheduled scan on a worker does.
        .route("/scan", post(metric_anomalies::run_scan))
        // The dry run of one monitor. Unlike `/scan` it asks for no working
        // copy: the file comes from the compile boundary and the semantic
        // model through `resolve_scan`, so any replica can answer.
        .route("/preview", post(metric_anomalies::preview_monitor))
        // Static `/status` (bulk) and `/{anomaly_id}/status` (single) differ in
        // segment count, so they don't compete for a match. Bulk triage writes
        // Postgres rows only — any replica can take it.
        .route("/status", post(metric_anomalies::update_status_bulk))
        .route(
            "/{anomaly_id}/status",
            post(metric_anomalies::update_status),
        )
        // Like `/preview`: the semantic model through `resolve_scan`, the
        // anomaly a Postgres row, the queries to the warehouse.
        .route(
            "/{anomaly_id}/explain",
            post(metric_anomalies::explain_anomaly),
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
/// Method `"*"`, as `route_fleet` declared them in `oxy-app`. The external tree
/// carries no declarations (its curated router drops them); every route there
/// falls to the FleetOk default, which is the truth here.
pub fn workspace_route_roles() -> &'static [RouteRoleDecl] {
    WORKSPACE_ROUTE_ROLES
}

/// A `const`, not a literal in the function: `fleet` is a `const fn`, and a
/// slice of its calls is promoted to `'static` only through a constant.
const WORKSPACE_ROUTE_ROLES: &[RouteRoleDecl] = &[
    fleet("/semantic/metric-tree"),
    fleet("/semantic/metric-tree/{measure_id}/sensitivity"),
    fleet("/semantic/metric-tree/predict"),
    fleet("/semantic/metric-tree/explain"),
    fleet("/semantic/metric-tree/opportunity"),
    fleet("/semantic/metric-tree/drill"),
    fleet("/semantic/metric-tree/time-dimensions"),
    fleet("/semantic/metric-tree/distribution"),
    fleet("/semantic/metric-tree/baseline"),
    fleet("/semantic/metric-tree/projection"),
    fleet("/semantic/monitors"),
    fleet("/semantic/anomalies"),
    fleet("/semantic/anomalies/scan"),
    fleet("/semantic/anomalies/preview"),
    fleet("/semantic/anomalies/status"),
    fleet("/semantic/anomalies/{anomaly_id}/status"),
    fleet("/semantic/anomalies/{anomaly_id}/explain"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use tower::ServiceExt;

    /// The routes came from `oxy-app`, where `route_fleet`'s type proved the
    /// role. Here the role is a declaration, so hold it: each route is DECLARED
    /// FleetOk exactly once — not merely unclassified, which `classify` also
    /// answers FleetOk and is what a dropped row would look like.
    ///
    /// The metric tree has a real outage behind it (oxy-hq/oxygen#878): the
    /// handlers used to read `semantics_scan_path()` directly, and every call
    /// 500'd on a stateless replica. The fix was the compile boundary, NOT an
    /// ide pin: viewing a metric tree is a read, and a read that needs the
    /// singleton is an HA bug. The scan reads `.monitor.yml` from the compiled
    /// revision, so it is FleetOk too.
    #[test]
    fn every_route_is_declared_fleet_ok_exactly_once() {
        let declared = workspace_route_roles();
        assert_eq!(declared.len(), 17, "10 metric-tree + monitors + 6 inbox");
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

    /// The declarations hold against the role registry the way the served
    /// router uses them: installed, then classified against a live URI. The
    /// scan and the metric tree are the two rows with outages behind them.
    #[test]
    fn the_installed_declarations_classify_fleet_ok() {
        use oxy_app_core::pods::role_manifest::declared_role_in;
        let decls: Vec<_> = workspace_route_roles()
            .iter()
            .map(|d| (d.method, format!("/api/{{workspace_id}}{}", d.path), d.role))
            .collect();
        for (method, uri) in [
            ("POST", "/api/d9830be4-c6a4/semantic/anomalies/scan"),
            ("GET", "/api/d9830be4-c6a4/semantic/anomalies"),
            ("POST", "/api/d9830be4-c6a4/semantic/anomalies/abc/explain"),
            ("GET", "/api/d9830be4-c6a4/semantic/monitors"),
            ("GET", "/api/d9830be4-c6a4/semantic/metric-tree"),
            (
                "GET",
                "/api/d9830be4-c6a4/semantic/metric-tree/revenue/sensitivity",
            ),
            ("POST", "/api/d9830be4-c6a4/semantic/metric-tree/baseline"),
            ("POST", "/api/d9830be4-c6a4/semantic/metric-tree/projection"),
        ] {
            assert_eq!(
                declared_role_in(&decls, method, uri),
                Some(RouteRole::FleetOk),
                "{method} {uri} must match a FleetOk declaration"
            );
        }
    }

    /// A declaration with no route behind it (or a route moved without its
    /// declaration) leaves the two lists disagreeing. TRACE, which no route
    /// here takes, answers 405 from a mounted path and 404 from an unmounted
    /// one, without running a handler.
    #[tokio::test]
    async fn every_declared_route_is_mounted() {
        let router = workspace_routes().with_state(oxy_app_core::app_state::bare_app_state());
        for decl in workspace_route_roles() {
            let uri = decl
                .path
                .replace("{measure_id}", "revenue")
                .replace("{anomaly_id}", "3f2504e0-4f89-11d3-9a0c-0305e82c3301");
            let status = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::TRACE)
                        .uri(&uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
                .status();
            assert_eq!(
                status,
                StatusCode::METHOD_NOT_ALLOWED,
                "{uri}: declared but not mounted"
            );
        }
    }

    /// The external tree must expose the FULL anomaly inbox — list, scan,
    /// status, explain — not just the read half. Standalone custom apps can't
    /// reach `/api`, so a missing verb here means the SDK's
    /// `client.anomalies.scan()` / `.explain()` 404 against `/external/api`
    /// while the same calls work in the IDE. (That `oxy-server` merges this
    /// tree is `served_router_tests`' business.)
    #[tokio::test]
    async fn the_external_tree_carries_the_full_anomaly_inbox() {
        let router = external_routes().with_state(oxy_app_core::app_state::bare_app_state());
        let id = "3f2504e0-4f89-11d3-9a0c-0305e82c3301";
        for path in [
            "/semantic/anomalies".to_string(),
            "/semantic/anomalies/scan".to_string(),
            "/semantic/anomalies/preview".to_string(),
            "/semantic/anomalies/status".to_string(),
            format!("/semantic/anomalies/{id}/status"),
            format!("/semantic/anomalies/{id}/explain"),
        ] {
            let status = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::TRACE)
                        .uri(&path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
                .status();
            assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "{path} not mounted");
        }
        // And nothing else: the metric tree and `/semantic/monitors` were never
        // on the external tree.
        let status = router
            .oneshot(
                Request::builder()
                    .method(Method::TRACE)
                    .uri("/semantic/monitors")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
