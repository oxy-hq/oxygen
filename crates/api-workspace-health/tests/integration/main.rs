//! `oxy-api-workspace-health`'s integration tests — one binary, per the
//! workspace rule (a new top-level `tests/*.rs` is another full link). Add a
//! case as a `mod`.
//!
//! These moved out of `oxy-app`'s `tests/platform/` with the workspace-health
//! surface and keep using `oxy-app`'s harness, included by path rather than
//! copied (it owns the per-run template database and the stray-database sweep).
//! They need nextest's process-per-test mode; `.config/nextest.toml` puts this
//! binary in `db-per-test`.

#[path = "../../../app/tests/common/mod.rs"]
mod common;

mod admin_staff_scope;
mod workspace_health_history;
