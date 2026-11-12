//! The world-model app's HTTP surface: the live event feed, foot traffic,
//! competitors and the LLM proxy the standalone world-model app calls (Pending 8
//! in `internal-docs/domain-boundaries.md`).
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
//! the compile boundary), and the proxy calls Anthropic with the workspace's key.
//!
//! The live feed's write half is `oxy_world_model::live_feed`, below `oxy-app`,
//! because the Toast webhook and the UniFi watcher still publish from there. The
//! read half is here: the per-pod buses, the tailer that fills them from
//! `world_model_events`, the reaper, and the SSE route. [`background`] starts the
//! tailer and the reaper, and registers the camera-domain bridge.
//!
//! The World Model *Graph* (`/semantic/world-model/*`) and peer cohorts are a
//! different surface and still in `oxy-app`.

// The same headroom every surface crate takes (see
// `crates/server/src/recursion_limit_guard.rs`): an async handler's future can
// exceed rustc's default query depth in the `--release` image build only.
#![recursion_limit = "256"]

mod camera_bridge;
mod competitors;
pub mod events;
mod foot_traffic;
mod foot_traffic_seed;
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
pub fn workspace_routes() -> Router<AppState> {
    shared_routes()
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
    ]
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
