//! Which pod may serve which route, checked against the real router: up from
//! `oxy-app`'s `tests/routing` group with the router (decompose LIFT,
//! 2026-11-23). An integration test of `oxy-server`'s library now, which is
//! the only crate that can build it.
//!
//! They belong together because they check each other: `route_role_derivation`
//! covers only `router/workspace.rs`, `global_route_roles` covers what that
//! parser structurally cannot see, and `fleet_canary` is the dynamic half of
//! both.
//!
//! **No database.** None of these opens one (no `common` harness, no
//! `establish_connection`, no `api_router(..)`), so `.config/nextest.toml`
//! carves `routing` out of this binary's `db-per-test` override, as it ran
//! ungrouped in `oxy-app`.
//!
//! One process-global to know about: `fleet_canary` flips
//! `oxy::workspace_fs_probe`'s owns-files flag and resets its leak counter. Those
//! cases carry `#[serial_test::serial]` — a guard private to one file excludes
//! nothing across module boundaries once the files share a binary.

mod fleet_canary;
mod fleet_routes_table;
mod global_route_roles;
mod route_role_derivation;
mod route_trailing_slash;
mod router_mount_collisions;
