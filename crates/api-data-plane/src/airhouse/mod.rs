//! Axum HTTP routes for per-user Airhouse credential access. Was
//! `airhouse::api` (the `rest` feature) until it left that crate.
//!
//! Routes (all under the host's auth middleware):
//! - `GET    /airhouse/version`               — the running Airhouse
//!   deployment's software version (global; no workspace). Read live from
//!   the deployment's public `/health`. Safe; no side effects.
//! - `GET    /airhouse/me/connection`         — wire endpoint + role,
//!   `is_provisioned` flag. Safe; no side effects.
//! - `POST   /airhouse/me/credentials`        — mint a fresh ephemeral
//!   credential. Each call writes an audit row on the airhouse side and
//!   counts against the per-SA mint quota, so the verb has to be POST
//!   (HTTP requires GET to be safe and idempotent).
//! - `POST   /airhouse/me/provision`          — ensure tenant + service
//!   account exist. Idempotent.
//! - `DELETE /airhouse/me/tokens/:username`   — revoke a single ephemeral
//!   credential the caller previously minted.

pub mod handlers;

use axum::Router;
use axum::routing::{delete, get, post};

/// Build the airhouse `/airhouse/me/*` route subtree.
///
/// The router is generic over the host application's state type — handlers
/// don't depend on app state. Mount with `app_router.merge(oxy_api_data_plane::airhouse::router())`.
pub fn router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/airhouse/version", get(handlers::get_version))
        .route("/airhouse/me/connection", get(handlers::get_connection))
        .route("/airhouse/me/credentials", post(handlers::get_credentials))
        .route("/airhouse/me/provision", post(handlers::provision))
        .route(
            "/airhouse/me/catalog-indexes",
            get(handlers::get_catalog_indexes).put(handlers::set_catalog_indexes),
        )
        .route(
            "/airhouse/me/tokens/{username}",
            delete(handlers::revoke_token),
        )
}
