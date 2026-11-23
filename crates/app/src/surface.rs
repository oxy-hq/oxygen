//! The surface prelude: everything an extracted HTTP surface crate
//! (`crates/api-<context>`) may take from `oxy-app`.
//!
//! Rule S4 in `internal-docs/domain-boundaries.md`. A surface crate imports
//! `oxy-app` through this module only, so `pub` deep inside `oxy-app` stops
//! meaning "some surface happened to need it" and the contract between the
//! platform runtime and the surfaces is one file you can read.
//!
//! What is left is what still lives in `oxy-app`. Decompose R1 (2026-11-23)
//! repointed every surface at the crate that holds the code (`oxy-app-core`,
//! `oxy-server-authz`, `oxy-workspace-runtime`, `oxy-compile-runtime`,
//! `oxy-previews`, …) and deleted the entries nothing named any more. The
//! remaining holders: `oxy-api-custom-apps` (the demo seed in production and in
//! its tests), `oxy-api-identity` (the seeded personas) and the tests of
//! `oxy-api-workspace-config` (the demo seed). The sandbox serve-tree fence went
//! down to `oxy_custom_apps::sandbox_reach` (decompose AG, 2026-11-23), and the
//! router helpers went up into `oxy-server` with the router (decompose LIFT,
//! 2026-11-23). The seed leaves with the CLI owners (decompose SD).
//!
//! Add an item here only if it is platform runtime that cannot live lower yet.
//! If it is someone's domain, lower it instead: a surface that needs another
//! context reaches it through a domain crate below `oxy-app` (rule S3), and
//! `tests/routing/surface_contract.rs` fails on any other `oxy_app::` path.

/// What the demo seed knows that a surface seeding its own rows needs: which
/// workspace the partner seed made for an org, and whether the database is one a
/// seed may touch.
pub mod seed {
    /// The personas the demo seed creates, which `oxy-api-identity`'s dev
    /// sign-in persona table is pinned against (in its tests).
    pub use crate::cli::commands::seed::seeded_persona_emails;
    pub use crate::cli::commands::seed::{SeedOptions, clear_demo, seed_demo_with};
    pub use crate::cli::commands::seed_partners::{is_local_db, seeded_workspace_id};
    pub use crate::cli::commands::seed_platform_grants::unscoped_app_operator_email;
}

// `testing` (the served router, the external auth stack and the publish-token
// scope middleware the surfaces' integration tests drive) and `roles` (the
// route-role installer that builds the real router) went up into `oxy-server`
// with the router (decompose LIFT, 2026-11-23): those tests dev-depend on
// `oxy-server` (`default-features = false`) and import `oxy_server::router`
// and `oxy_server::role_manifest`.
