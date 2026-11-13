//! The world-model app's HTTP surface: the live event feed, foot traffic,
//! competitors and the LLM proxy the standalone world-model app calls, and the
//! IDE's World Model Graph and peer cohorts (Pending 8 in
//! `internal-docs/domain-boundaries.md`).
//!
//! Two trees mount it, as they mounted these handlers when they lived in
//! `oxy-app`'s `build_workspace_routes` and `build_external_workspace_routes`:
//!
//! - [`workspace_routes`] rides the workspace seam, merged INSIDE the
//!   `/api/{workspace_id}` nest, so it inherits the session auth stack and
//!   `workspace_middleware`.
//! - [`external_routes`] rides the external seam, merged into the curated
//!   `/external/api/{workspace_id}` tree before its API-key-only auth, wide-open
//!   CORS and workspace context layers. It is the same four routes plus the LLM
//!   proxy, which only standalone apps call.
//!
//! Every route is FleetOk: the feed reads Postgres, foot traffic and
//! competitors call their vendors (and read `foot_traffic_by_store.yml` through
//! the compile boundary), the proxy calls Anthropic with the workspace's key,
//! and the graph and cohort routes read the semantic model through the compile
//! boundary (working copy second, where the node holds one) and run on the
//! workspace's warehouses.
//!
//! The live feed's write half is `oxy_world_model::live_feed`, below `oxy-app`,
//! because the Toast webhook and the UniFi watcher still publish from there. The
//! read half is here: the per-pod buses, the tailer that fills them from
//! `world_model_events`, the reaper, and the SSE route. [`background`] starts the
//! tailer and the reaper, and registers the camera-domain bridge.
//!
//! The World Model *Graph* (`/semantic/world-model*`, [`graph`]) and peer
//! cohorts (`/semantic/cohort`, [`cohort`]) ride the workspace tree only. Their
//! cores are `oxy_world_model::{graph, cohort}`, shared with their customer-app
//! twins ([`customer_app`]: `/projects/{project_id}/semantic/{world-model*,cohort}`),
//! which ride the public seam behind the customer-app gate.

// The same headroom every surface crate takes (see
// `crates/server/src/recursion_limit_guard.rs`): an async handler's future can
// exceed rustc's default query depth in the `--release` image build only.
#![recursion_limit = "256"]

mod camera_bridge;
pub mod cohort;
mod competitors;
pub mod customer_app;
pub mod events;
mod foot_traffic;
mod foot_traffic_seed;
pub mod graph;
mod llm_proxy;

use axum::Router;
use axum::routing::{get, post};
use oxy_app::surface::seams::BackgroundSeam;
use oxy_app_core::AppState;
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

/// The four routes both trees serve, relative to `/{workspace_id}`.
fn shared_routes() -> Router<AppState> {
    Router::new()
        // The live feed reads Postgres only: publishers append to
        // `world_model_events` and every pod tails that table onto its own bus,
        // so any replica serves a subscriber.
        .route("/world-model/events", get(events::world_model_events_sse))
        .route(
            "/world-model/foot-traffic/current",
            post(foot_traffic::foot_traffic_current_batch),
        )
        .route(
            "/world-model/foot-traffic/radar",
            post(foot_traffic::foot_traffic_radar_batch),
        )
        .route(
            "/world-model/competitors",
            post(competitors::get_competitors),
        )
}

/// The workspace seam's share: merged inside `/api/{workspace_id}`.
///
/// The graph and cohort routes are the IDE's, so they ride this tree only — not
/// the external one, whose API-key auth and open CORS they were never under.
pub fn workspace_routes() -> Router<AppState> {
    shared_routes()
        // The World Model Graph. The handlers take `WorkspaceManagerReadOnly`
        // and resolve the model through `graph::source` (compile boundary
        // first), as the metric tree does, so any replica serves them.
        .route("/semantic/world-model", get(graph::get_world_model))
        .route(
            "/semantic/world-model/instances",
            get(graph::get_world_model_instances),
        )
        .route(
            "/semantic/world-model/filter-instances",
            get(graph::get_world_model_filter_instances),
        )
        .route(
            "/semantic/world-model/filter-counts",
            post(graph::post_world_model_filter_counts),
        )
        .route(
            "/semantic/world-model/instance-detail",
            get(graph::get_world_model_instance_detail),
        )
        .route(
            "/semantic/world-model/measure-breakdown",
            get(graph::get_world_model_measure_breakdown),
        )
        // Peer cohorts — the World Model instance panel's "compared to peers"
        // section. IDE twin of the customer-app mount in
        // `public_routes`; the two share `oxy_world_model::cohort`'s wire types and
        // differ only in how a caller gets in and whether the population is
        // reach-scoped. Not beside the metric-tree routes: those are a
        // sub-router so `metric_tree_probe` times exactly them, and a cohort
        // resolution's latency would pollute that signal.
        .route("/semantic/cohort", post(cohort::post_cohort))
}

/// The external seam's share: merged inside `/external/api/{workspace_id}`.
pub fn external_routes() -> Router<AppState> {
    shared_routes()
        // Thin LLM passthrough for the standalone voice assistant: takes an
        // Anthropic Messages payload (system + messages + optional tools),
        // resolves the workspace Anthropic key server-side, and returns the raw
        // content blocks — so the app never ships a provider key in the browser.
        .route(
            "/world-model/llm/messages",
            post(llm_proxy::proxy_llm_messages),
        )
}

/// The public seam's share: the customer-app twins, merged beside `oxy-app`'s
/// public tree under `/api`, outside the auth stack. A bundle authenticates
/// inline at the customer-app gate (`crate::customer_app`).
pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project_id}/semantic/world-model",
            get(customer_app::world_model::get_world_model),
        )
        .route(
            "/projects/{project_id}/semantic/world-model/instances",
            get(customer_app::world_model::get_world_model_instances),
        )
        .route(
            "/projects/{project_id}/semantic/world-model/measure-breakdown",
            get(customer_app::world_model::get_measure_breakdown),
        )
        // Peer cohorts. Scoped to the caller's reach, so unlike the
        // population-wide IDE twin it refuses rather than answers when the
        // entity is not bound to the locations registry.
        .route(
            "/projects/{project_id}/semantic/cohort",
            post(customer_app::cohort::post_cohort),
        )
}

/// Pod placement for [`public_routes`], relative to `/api`. All four read the
/// compile boundary and Postgres only: a custom app must not go down with the
/// Factory.
pub fn public_route_roles() -> &'static [RouteRoleDecl] {
    &[
        RouteRoleDecl {
            method: "GET",
            path: "/projects/{project_id}/semantic/world-model",
            role: RouteRole::FleetOk,
        },
        RouteRoleDecl {
            method: "GET",
            path: "/projects/{project_id}/semantic/world-model/instances",
            role: RouteRole::FleetOk,
        },
        RouteRoleDecl {
            method: "GET",
            path: "/projects/{project_id}/semantic/world-model/measure-breakdown",
            role: RouteRole::FleetOk,
        },
        RouteRoleDecl {
            method: "POST",
            path: "/projects/{project_id}/semantic/cohort",
            role: RouteRole::FleetOk,
        },
    ]
}

/// Pod placement for [`workspace_routes`], relative to the workspace tree.
/// The external tree carries no declarations (its curated router drops them);
/// every route there falls to the FleetOk default, which is the truth here.
pub fn workspace_route_roles() -> &'static [RouteRoleDecl] {
    &[
        RouteRoleDecl {
            method: "GET",
            path: "/world-model/events",
            role: RouteRole::FleetOk,
        },
        RouteRoleDecl {
            method: "POST",
            path: "/world-model/foot-traffic/current",
            role: RouteRole::FleetOk,
        },
        RouteRoleDecl {
            method: "POST",
            path: "/world-model/foot-traffic/radar",
            role: RouteRole::FleetOk,
        },
        RouteRoleDecl {
            method: "POST",
            path: "/world-model/competitors",
            role: RouteRole::FleetOk,
        },
        // The graph and cohort routes: the scan root resolves through the
        // compile boundary first, working copy second, and warehouse execution
        // needs only config + secrets. No `.git`, no state dir, and the branch
        // a request reads is pinned by `workspace_middleware`, not here.
        RouteRoleDecl {
            method: "GET",
            path: "/semantic/world-model",
            role: RouteRole::FleetOk,
        },
        RouteRoleDecl {
            method: "GET",
            path: "/semantic/world-model/instances",
            role: RouteRole::FleetOk,
        },
        RouteRoleDecl {
            method: "GET",
            path: "/semantic/world-model/filter-instances",
            role: RouteRole::FleetOk,
        },
        RouteRoleDecl {
            method: "POST",
            path: "/semantic/world-model/filter-counts",
            role: RouteRole::FleetOk,
        },
        RouteRoleDecl {
            method: "GET",
            path: "/semantic/world-model/instance-detail",
            role: RouteRole::FleetOk,
        },
        RouteRoleDecl {
            method: "GET",
            path: "/semantic/world-model/measure-breakdown",
            role: RouteRole::FleetOk,
        },
        RouteRoleDecl {
            method: "POST",
            path: "/semantic/cohort",
            role: RouteRole::FleetOk,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The graph and cohort routes came from `oxy-app`, where `route_fleet`'s
    /// type proved the role. Here the role is a declaration, so hold it: each
    /// is DECLARED FleetOk — not merely unclassified, which also reads FleetOk
    /// and is what a dropped row would look like. A read pinned to the
    /// singleton would stop the World Model panel whenever the ide restarts.
    /// That no handler here reaches for the working copy is held by
    /// `graph::source`'s `nothing_here_reads_the_working_copy_directly` and
    /// `oxy-app`'s `undeclared_mounts_stay_diskless`.
    #[test]
    fn the_graph_and_cohort_routes_are_declared_fleet_ok() {
        let declared = workspace_route_roles();
        for (method, path) in [
            ("GET", "/semantic/world-model"),
            ("GET", "/semantic/world-model/instances"),
            ("GET", "/semantic/world-model/filter-instances"),
            ("POST", "/semantic/world-model/filter-counts"),
            ("GET", "/semantic/world-model/instance-detail"),
            ("GET", "/semantic/world-model/measure-breakdown"),
            ("POST", "/semantic/cohort"),
        ] {
            let found: Vec<_> = declared
                .iter()
                .filter(|d| d.method == method && d.path == path)
                .collect();
            assert_eq!(
                found.len(),
                1,
                "{method} {path}: declared {} times",
                found.len()
            );
            assert_eq!(found[0].role, RouteRole::FleetOk, "{method} {path}");
        }
    }
    /// The customer-app twins came from `oxy-app`'s public tree, where
    /// `route_fleet` declared them; here the declaration is a list, so hold it.
    /// Each is declared FleetOk exactly once — not merely unclassified, which
    /// also reads FleetOk and is what a dropped row would look like.
    #[test]
    fn the_customer_app_twins_are_declared_fleet_ok_exactly_once() {
        let declared = public_route_roles();
        assert_eq!(declared.len(), 4);
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
            assert_eq!(
                decl.role,
                RouteRole::FleetOk,
                "{} {}",
                decl.method,
                decl.path
            );
        }
    }

    /// A declaration with no route behind it (or a route moved without its
    /// declaration) leaves the two lists disagreeing. TRACE, which no route
    /// here takes, answers 405 from a mounted path and 404 from an unmounted
    /// one, without running a handler.
    #[tokio::test]
    async fn every_declared_twin_is_mounted() {
        use axum::body::Body;
        use axum::http::{Method, Request, StatusCode};
        use tower::ServiceExt;

        let router = public_routes().with_state(oxy_app::surface::bare_app_state());
        for decl in public_route_roles() {
            let uri = decl
                .path
                .replace("{project_id}", "00000000-0000-0000-0000-000000000001");
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::TRACE)
                        .uri(&uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "{} is declared but not mounted",
                decl.path
            );
        }
    }
}

/// The live feed's process-lifetime jobs, started by `api_router` on every
/// serve process (not singleton-gated: each pod fills its own buses).
///
/// One seam rather than three because they are one feature: without the tailer
/// a webhook that lands on another replica reaches nobody here, and without the
/// bridge no camera event is published at all.
pub fn background() -> Vec<BackgroundSeam> {
    vec![BackgroundSeam {
        name: "world_model_live_feed",
        start: |_deps| {
            // Bridge camera domain events (compliance ingest, health
            // transitions) onto the feed.
            oxy_cameras::service::events::set_sink(Box::new(
                camera_bridge::publish_camera_domain_event,
            ));
            // Publishers append to `world_model_events`; the tailer turns those
            // rows back into SSE frames on THIS pod, so it runs everywhere, the
            // publishing pod included. The reaper trims the retained window.
            events::spawn_world_model_tailer();
            events::spawn_world_model_reaper();
            None
        },
    }]
}
