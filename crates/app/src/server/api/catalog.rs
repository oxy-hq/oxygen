//! `GET /api/_catalog` — the route table, served by the deployment that owns it.
//!
//! # Why this exists
//!
//! `oxyc` (the TypeScript CLI, `sdk/cli`) is the discovery surface for anyone
//! who has a token and nothing else: no checkout, no doc site, no Swagger UI
//! they can browse. It has to be able to answer "what can I call here?" — and
//! it cannot generate the answer, because the answer comes from a build-time
//! walk of this crate's router source (`build_route_catalog.rs`).
//!
//! The Rust `oxy api --routes` answered it from the table baked into the
//! binary. That has one flaw this endpoint fixes: the baked table describes
//! what the BINARY could mount, and several mounts are mode-conditional
//! (`/setup/*` and the git routes exist only in local mode), so a listed path
//! could still 404 on the deployment in front of you. Asking the deployment
//! is strictly more truthful, and it is the only form of the question that
//! stays correct as a caller moves between local, dev and production.
//!
//! # Why it is authenticated
//!
//! It sits on the protected router. A complete route table for a multi-tenant
//! SaaS — admin surfaces, partner console, billing, every path parameter — is
//! a reconnaissance gift, and there is no caller who needs it before they have
//! a token: the CLI holds one for every other command it runs. The client
//! caches per host, so the cost is one call an hour, not one per command.
//!
//! # Scope
//!
//! Whatever `server::route_catalog` covers: the `/api` and `/external/api`
//! surfaces. See that module for what is deliberately outside those bounds
//! (the custom-app bundle tree, the worker health port, the internal loopback
//! router).

use axum::Json;
use axum::extract::Query;
use serde::{Deserialize, Serialize};

use crate::server::route_catalog::{RouteCatalog, RouteDescription};

/// `?filter=` narrows the table server-side.
#[derive(Deserialize, Default)]
pub struct CatalogQuery {
    /// Substring matched against method, path, surface and description.
    pub filter: Option<String>,
}

/// The document `oxyc` caches per host.
#[derive(Serialize)]
pub struct CatalogResponse {
    /// Every route on the `/api` and `/external/api` surfaces.
    pub routes: Vec<RouteDescription>,
    /// Surfaces in display order, each with the credential it expects.
    pub surfaces: Vec<CatalogSurface>,
    /// The build this table was generated from, so a stale client cache and a
    /// redeployed server can be told apart without guessing from timestamps.
    pub version: &'static str,
}

#[derive(Serialize)]
pub struct CatalogSurface {
    pub id: &'static str,
    pub label: &'static str,
    pub credential: &'static str,
}

/// The route table for this deployment, with what each endpoint does and which
/// credential its surface expects.
///
/// Consumed by `oxyc routes` / `oxyc schema`; the OpenAPI document at
/// `/apidoc/openapi.json` carries the request/response schemas for the
/// curated subset that has them.
///
/// `?filter=<substring>` narrows it here rather than in the client. The whole
/// table is ~670 routes and a few hundred KB of prose; a caller looking for
/// three of them should not have to download all of it, and a filtered request
/// is also the shape a cache-less caller wants.
///
/// `catalog` is bound at mount time (`build_catalog_routes`): the table is
/// generated above this crate, by `oxy-route-catalog`. Its tests live there too,
/// beside the table they read.
pub async fn get_catalog(
    catalog: RouteCatalog,
    Query(query): Query<CatalogQuery>,
) -> Json<CatalogResponse> {
    let needle = query
        .filter
        .as_deref()
        .map(str::trim)
        .filter(|f| !f.is_empty());
    Json(CatalogResponse {
        routes: catalog
            .search(needle)
            .into_iter()
            .map(|route| catalog.describe(route))
            .collect(),
        surfaces: catalog
            .surfaces
            .iter()
            .map(|(id, label, credential)| CatalogSurface {
                id,
                label,
                credential,
            })
            .collect(),
        version: env!("CARGO_PKG_VERSION"),
    })
}
