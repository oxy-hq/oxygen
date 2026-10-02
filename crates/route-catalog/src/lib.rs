//! The HTTP route table, generated.
//!
//! `oxy-app` owns the [`GeneratedRoute`] / [`RouteCatalog`] types and the
//! `GET /api/_catalog` handler; this crate owns the DATA, produced by
//! `build.rs` from the router source of `oxy-app` and every surface crate.
//! `oxy-server` hands [`catalog()`] to `oxy-app` through `SurfaceSeams`.
//!
//! Why the split: the build script must read every surface crate, and as
//! `oxy-app`'s build script that made each surface edit recompile `oxy-app`.
//! Above both, an edit re-runs only this script
//! (`internal-docs/domain-boundaries.md` S7).
//!
//! The completeness tests live here, beside the table they read.

use oxy_app::server::route_catalog::{GeneratedRoute, RouteCatalog};

include!(concat!(env!("OUT_DIR"), "/route_catalog_generated.rs"));

/// The generated table, for the composition root to hand to `oxy-app`.
pub fn catalog() -> RouteCatalog {
    RouteCatalog {
        routes: GENERATED_ROUTES,
        surfaces: GENERATED_SURFACES,
    }
}

#[cfg(test)]
mod tests;
