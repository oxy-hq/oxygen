//! Oxy Application - CLI and HTTP Server
//!
//! This crate provides both command-line interface and HTTP server functionality
//! for Oxy, integrating domain crates (oxy-auth, agentic-*, oxy-workflow, etc.)

// The deeply-nested async futures in the HTTP layer (Axum handlers + the
// custom-app function host's chained `with_db_timeout`/connector futures)
// exceed the default type-layout query depth of 128. rustc's own suggestion.
#![recursion_limit = "256"]

// `BUILD_SHA` and the build identity `host` re-exported (`server::build_identity`),
// with the build script that stamps them, went up into `oxy-server` in
// decompose BID (2026-11-25): `oxy_server::{BUILD_SHA, build_identity}`.

pub mod agentic_wiring;
pub mod host;
pub mod server;
// `cli` (the commands) and `managed_connection_resolver` went up into
// `oxy-server` in decompose CLI (2026-11-25); `cli::render`, the dead
// `CliRenderer`, was deleted in decompose Z0.
// `surface`, the prelude the `crates/api-*` surfaces took from oxy-app, is gone:
// the router helpers went up into oxy-server in decompose LIFT, and the demo
// seed, its last entry, in decompose SD (2026-11-24).

// The `emails` and `api` aliases went in decompose RPT (2026-11-24): nothing
// named them once `oxy-server`'s tests imported `oxy_app_core::emails` and
// `oxy_server_authz` directly.
