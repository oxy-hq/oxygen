//! Oxy Application - CLI and HTTP Server
//!
//! This crate provides both command-line interface and HTTP server functionality
//! for Oxy, integrating domain crates (oxy-auth, agentic-*, oxy-workflow, etc.)

// The deeply-nested async futures in the HTTP layer (Axum handlers + the
// custom-app function host's chained `with_db_timeout`/connector futures)
// exceed the default type-layout query depth of 128. rustc's own suggestion.
#![recursion_limit = "256"]

/// The commit this binary was built from, short form — `"dev"` for a local
/// build, since `build.rs` has only what CI puts in `GITHUB_SHA`.
///
/// Lives here rather than in a build script on `oxy` so that the platform
/// library stays build-script-free: a `rustc-env` on `oxy` would put a
/// per-commit input at the root of the graph and rebuild `oxy` plus its nine
/// direct dependents on every CI commit, where this crate is a near-leaf and
/// already pays that cost (internal-docs/rust-build-performance.md).
pub const BUILD_SHA: &str = env!("GIT_HASH");

pub mod agentic_wiring;
pub mod cli;
pub mod host;
pub mod managed_connection_resolver;
pub mod server;
// `surface`, the prelude the `crates/api-*` surfaces took from oxy-app, is gone:
// the router helpers went up into oxy-server in decompose LIFT, and the demo
// seed, its last entry, in decompose SD (2026-11-24).

// The `emails` and `api` aliases went in decompose RPT (2026-11-24): nothing
// named them once `oxy-server`'s tests imported `oxy_app_core::emails` and
// `oxy_server_authz` directly.
