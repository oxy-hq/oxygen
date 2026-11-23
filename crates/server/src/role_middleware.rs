//! The role middleware: `oxy-app-core`'s `pods::role_middleware` since
//! 2026-11-22 (decompose G3), re-exported here so the router, the prelude and
//! `host` keep this path.
//!
//! Its tests stay here: they classify against the live route declarations,
//! which only the router (`role_manifest`'s test installer) can build.

pub use oxy_app_core::pods::role_middleware::*;

#[cfg(test)]
#[path = "role_middleware_tests.rs"]
mod tests;
