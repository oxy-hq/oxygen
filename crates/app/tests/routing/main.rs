//! Which pod may serve which route, and what a route is allowed to touch.
//!
//! One binary for the whole group; see `tests/authz/main.rs` for why. Add a case
//! as a `mod` here rather than a new `tests/*.rs`.
//!
//! The cases that build the real router — `fleet_canary`, `fleet_routes_table`,
//! `global_route_roles`, `route_role_derivation`, `route_trailing_slash` and
//! `router_mount_collisions` — went up into `oxy-server`'s integration group
//! (`tests/integration/routing/`) with the router in decompose LIFT
//! (2026-11-23). What is left scans source and manifests.
//!
//! **Ungrouped in `.config/nextest.toml`, deliberately.** Nothing here opens a
//! database — no [`common`] harness, no `establish_connection`, no `api_router(..)`
//! — so the binary runs at full parallelism alongside everything else.
//! `authz::shared_db_registry` scans this directory (it is in that file's
//! `MIXED_BINARIES`) and fails the build the day a case here starts reaching the
//! shared `OXY_DATABASE_URL` without being pinned into `serial-db`.

// (`fleet_canary`'s meta-route test, whose handler's future needed
// `#![recursion_limit = "256"]` here, moved to `oxy-api-workspaces`'
// integration group on 2026-11-16, and the attribute with it.)

mod app_growth;
mod base_boundary;
mod config_write_routes;
mod env_reads;
mod handler_spawns;
mod paginated_links_keep_the_api_prefix;
mod sentry_hub_scan;
mod surface_contract;
mod undeclared_mounts_stay_diskless;
