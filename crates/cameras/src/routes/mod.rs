//! Axum HTTP routes for the camera fleet domain.
//!
//! Two subtrees, two mounting points:
//!
//! - **Edge tree** ([`router`]) — `/control/*` routes that the Python
//!   edge worker calls. The device-token middleware verifies a
//!   per-device JWT (signed with the device's HMAC secret from
//!   `device_registry`), resolves the active `device_claims` row, and
//!   injects the [`EdgeContext`](crate::auth::EdgeContext) (with
//!   `workspace_id`) into request extensions. Mounted at the *root* of
//!   the app router because edge boxes don't know their workspace_id
//!   — the JWT alone identifies them.
//!
//! - **Operator workspace tree** ([`workspace_routes`]) — user-facing
//!   endpoints for managing sites / cameras / edge boxes / UniFi
//!   onboarding. Mounted by the app crate *under
//!   `/{workspace_id}/...`* alongside the rest of
//!   `build_workspace_routes`, which puts them behind the standard
//!   user-session + workspace-context middleware. Service-layer fns
//!   double-check that any referenced resource (site, camera) belongs
//!   to the URL's workspace_id — protects against a caller in
//!   workspace A guessing a `site_id` from workspace B.

pub mod dto;
pub mod edge;
pub mod errors;
pub mod fleet;
pub mod mtx_auth;
pub mod operator;

use axum::{Extension, Router, middleware};
use sea_orm::DatabaseConnection;

use crate::auth::require_device_token;

/// Edge-facing `/control/*` tree. Mount at the top level of the app
/// router (no workspace prefix — the device token resolves
/// `workspace_id` implicitly).
///
/// Also includes the MediaMTX HTTP-auth callback endpoint
/// (`/control/mtx-auth`) which is *NOT* gated by the device-token
/// middleware — MTX itself calls back without a bearer. The trust
/// boundary on that route is the tailnet ACL (only `tag:edge-box`
/// can reach `tag:oxy-backend`, see `internal-docs/tailscale-acl.json`)
/// plus the HMAC validation inside the handler.
///
/// Mount with something like:
///
/// ```ignore
/// let db = agentic_state.db.clone();
/// app_router = app_router.merge(oxy_cameras::routes::router::<AppState>(db));
/// ```
pub fn router<S>(db: DatabaseConnection) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let edge_routes = edge::routes::<S>().route_layer(middleware::from_fn(require_device_token));
    let mtx_auth_routes = mtx_auth::routes::<S>();
    // Fleet routes carry their own auth (shared secret for factory-
    // enroll, HMAC for announce) so they don't sit behind
    // require_device_token. Devices that hit /fleet/* don't yet
    // have a JWT-issuing identity row to authenticate against.
    let fleet_routes = fleet::routes::<S>();
    Router::new()
        .merge(edge_routes)
        .merge(mtx_auth_routes)
        .merge(fleet_routes)
        .layer(Extension(db))
}

/// Operator-facing workspace-scoped tree. Designed to be merged into
/// the app crate's `build_workspace_routes`, which nests it under
/// `/{workspace_id}` and applies `workspace_middleware` (cloud) or
/// `local_context_middleware` (local).
///
/// Mount with something like:
///
/// ```ignore
/// // crates/server/src/router/workspace.rs
/// .merge(oxy_cameras::routes::workspace_routes::<AppState>(db.clone()))
/// ```
///
/// Each route here trusts `workspace_id` from the URL — the upstream
/// `workspace_middleware` has already verified the caller has access
/// to it. Each handler then re-checks that the resource it touches
/// belongs to that workspace (defense in depth against URL-guessing).
pub fn workspace_routes<S>(db: DatabaseConnection) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    operator::routes::<S>().layer(Extension(db))
}

/// Live-stream subtree for the EXTERNAL API surface. Nested by the app
/// crate under `/{workspace_id}/world-model/camera-stream` behind
/// API-key-only auth.
pub fn external_stream_routes<S>(db: DatabaseConnection) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    operator::external_stream_routes::<S>().layer(Extension(db))
}

/// Clip-playback URL for the EXTERNAL API surface. Merged by the app crate
/// into the `/{workspace_id}` external tree behind API-key-only auth, so
/// customer apps can play archived evidence/compliance clips. No DB needed —
/// the handler only mints a presigned S3 GET after a workspace-prefix check.
pub fn external_clip_routes<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    operator::external_clip_routes::<S>()
}
