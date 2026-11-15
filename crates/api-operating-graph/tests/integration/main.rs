//! `oxy-api-operating-graph`'s integration tests — one binary, per the
//! workspace rule (add a case as a `mod` here, never a new top-level
//! `tests/*.rs`).
//!
//! These moved out of `oxy-app`'s `tests/platform/` with the operating-graph
//! and work-item handlers on 2026-11-14: `oxy-app` cannot depend on this crate,
//! so its test binaries can no longer name `oxy_api_operating_graph::…`.
//!
//! Both modules reuse `oxy-app`'s per-test harness, included by path rather
//! than copied: it owns the per-run template database and the stray-database
//! sweep, and a second copy would drift from both. Each test gets its own
//! database cloned from the template, so this binary sits in nextest's
//! `db-per-test` group (`.config/nextest.toml`), as the `platform` binary did.
//! `route_roles` opens no database.

#[path = "../../../app/tests/common/mod.rs"]
mod common;

mod operating_graph;
mod route_roles;
mod work_item_gates;
