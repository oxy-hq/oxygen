//! `oxy-api-observability`'s integration tests — one binary, per the workspace
//! rule (a new top-level `tests/*.rs` is another full link). Add a case as a
//! `mod`.
//!
//! The DB-backed ones moved out of `oxy-app`'s `tests/platform/` with the
//! handlers they drive and keep using `oxy-app`'s harness, included by path
//! rather than copied (it owns the per-run template database and the
//! stray-database sweep). Each test gets its own database, so
//! `.config/nextest.toml` puts this binary in `db-per-test`.

#[path = "../../../test-utils/harness/mod.rs"]
mod common;

// Rehomed from `oxy-app`'s `platform` group with Admin → Metrics (admin split,
// PR 1). Its own database per test.
mod admin_staff_scope_llm_usage;
