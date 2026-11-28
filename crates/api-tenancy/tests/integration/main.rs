//! `oxy-api-tenancy`'s integration tests — one binary, per the workspace rule
//! (a new top-level `tests/*.rs` is another full link). Add a case as a `mod`.

//! The DB-backed ones moved out of `oxy-app`'s `tests/platform/` with the
//! tenancy handlers and keep using `oxy-app`'s harness, included by path rather
//! than copied (it owns the per-run template database and the stray-database
//! sweep). Like `oxy-api-frontline`'s, they need nextest's process-per-test
//! mode; `.config/nextest.toml` puts this binary in `db-per-test`.

#[path = "../../../test-utils/harness/mod.rs"]
mod common;

mod admin_factory_access;
mod admin_staff_scope_directories;
mod onboarding_write_guards;
mod one_workspace_per_org;
mod org_default_workspace;
// From `oxy-app`'s `authz` group (decompose T-small, 2026-11-24). It does not
// use the harness: it calls `establish_connection()` on the SHARED database, so
// `.config/nextest.toml` pins it into `serial-db`, out of `db-per-test`.
mod org_invitations;
mod workspace_repository_columns;
