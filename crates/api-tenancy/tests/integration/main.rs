//! `oxy-api-tenancy`'s integration tests — one binary, per the workspace rule
//! (a new top-level `tests/*.rs` is another full link). Add a case as a `mod`.

//! The DB-backed ones moved out of `oxy-app`'s `tests/platform/` with the
//! tenancy handlers and keep using `oxy-app`'s harness, included by path rather
//! than copied (it owns the per-run template database and the stray-database
//! sweep). Like `oxy-api-frontline`'s, they need nextest's process-per-test
//! mode; `.config/nextest.toml` puts this binary in `db-per-test`.

#[path = "../../../app/tests/common/mod.rs"]
mod common;

mod admin_route_roles;
mod admin_staff_scope_directories;
mod onboarding_route_roles;
mod org_default_workspace;
