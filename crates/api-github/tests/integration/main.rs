//! `oxy-api-github`'s integration tests — one binary, per the workspace rule
//! (a new top-level `tests/*.rs` is another full link). Add a case as a `mod`.
//!
//! The DB-backed ones use `oxy-app`'s harness, included by path rather than
//! copied (it owns the per-run template database and the stray-database
//! sweep). They need nextest's process-per-test mode; `.config/nextest.toml`
//! puts this binary in `db-per-test`.
//!
//! Keep each module a top-level file here: `oxy-app`'s `shared_db_registry`
//! scans this directory's `.rs` files, not its subdirectories.
//!
//! The `installation_*` modules are the GitHub App installation-ownership
//! cases (#3753); `installation_fixture` says what they hold to.

#[path = "../../../app/tests/common/mod.rs"]
mod common;

mod installation_attach_route;
mod installation_callback;
mod installation_fixture;
mod installation_new_url;
