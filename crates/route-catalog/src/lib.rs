//! The HTTP route table, generated.
//!
//! This crate owns the [`GeneratedRoute`] / [`RouteCatalog`] types (since
//! decompose RC, 2026-11-23; `oxy-app` held them before) and the DATA, while
//! `oxy-server` owns the `GET /api/_catalog` handler (since 2026-11-16). The
//! data is produced by `build.rs` from the router source of `oxy-app`,
//! every surface crate and the binary's `core_routes`. `oxy-server` binds
//! [`catalog()`] into its handler and hands the mount to `oxy-app` through
//! `SurfaceSeams::catalog`.
//!
//! Why the split: the build script must read every surface crate, and as
//! `oxy-app`'s build script that made each surface edit recompile `oxy-app`.
//! Above both, an edit re-runs only this script
//! (`internal-docs/domain-boundaries.md` S7).
//!
//! The completeness tests live here, beside the table they read; the
//! handler's are `oxy-server`'s `catalog_tests`, and the ones that need the
//! real router are in `oxy-server`'s integration group (`route_catalog::`,
//! since decompose RT).

mod route_catalog;

pub use route_catalog::{
    GeneratedRoute, RouteCatalog, RouteDescription, path_parameters, searchable_fields,
};

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
