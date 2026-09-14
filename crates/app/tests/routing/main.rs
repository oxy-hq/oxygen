//! Which pod may serve which route, and what a route is allowed to touch.
//!
//! One binary for the whole group; see `tests/authz/main.rs` for why. Add a case
//! as a `mod` here rather than a new `tests/*.rs`.
//!
//! These seven were seven separate top-level targets — seven full links of
//! DuckDB + DataFusion + Arrow + the AWS SDK — for tests that share one subject:
//! `role_manifest`'s classification of a route, the router shape that
//! classification is derived from, and the runtime probe that catches what the
//! static checks missed. They belong together because they check each other:
//! `route_role_derivation` covers only `router/workspace.rs`, `global_route_roles`
//! covers what that parser structurally cannot see, and `fleet_canary` is the
//! dynamic half of both.
//!
//! **Ungrouped in `.config/nextest.toml`, deliberately.** Nothing here opens a
//! database — no [`common`] harness, no `establish_connection`, no `api_router(..)`
//! — so the binary runs at full parallelism alongside everything else.
//! `authz::shared_db_registry` scans this directory (it is in that file's
//! `MIXED_BINARIES`) and fails the build the day a case here starts reaching the
//! shared `OXY_DATABASE_URL` without being pinned into `serial-db`.
//!
//! One process-global to know about: `fleet_canary` flips
//! `oxy::workspace_fs_probe`'s owns-files flag and resets its leak counter. Those
//! cases carry `#[serial_test::serial]` — a guard private to one file excludes
//! nothing across module boundaries once the files share a binary.

mod config_write_routes;
mod fleet_canary;
mod global_route_roles;
mod paginated_links_keep_the_api_prefix;
mod route_role_derivation;
mod route_trailing_slash;
mod router_mount_collisions;
mod undeclared_mounts_stay_diskless;
