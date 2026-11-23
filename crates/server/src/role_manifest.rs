//! Route roles: `oxy-app-core`'s `pods::role_manifest` since 2026-11-22
//! (decompose G3), re-exported here so the router, the prelude, `host` and the
//! `tests/routing` cases keep this path.
//!
//! What stays is the test installer: it builds the real router to read its
//! declarations back, so it lives beside the router, and so do the tests that
//! classify against it (`role_manifest_tests.rs`).

pub use oxy_app_core::pods::role_manifest::*;

/// The TEST path: builds the protected router purely to read its declarations
/// back. Nothing in `src/` calls this — a test that classifies must install
/// first, or `classify` answers `FleetOk` for everything.
pub fn install_route_declarations_for_tests() {
    install_route_declarations_for_tests_with(Vec::new());
}

/// The same, plus declarations a surface crate supplies at the composition
/// seam. `oxy-app` cannot depend on those crates, so a test that needs their
/// routes classified has to hand them in — otherwise the route is simply absent
/// and `classify` answers with the FleetOk default, which is the failure this
/// whole file exists to make impossible.
pub fn install_route_declarations_for_tests_with(extra: Vec<(&'static str, String, RouteRole)>) {
    let mut decls = crate::router::route_declarations();
    decls.extend(api_prefixed(extra));
    install_declarations(decls);
}

#[cfg(test)]
#[path = "role_manifest_tests.rs"]
mod tests;
