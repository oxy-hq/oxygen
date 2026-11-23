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
//! remaining holders: `oxy-api-custom-apps` (the demo seed and the sandbox
//! serve-tree fence in production; the router helpers in its tests), and the
//! tests of the eight crates that keep `oxy-app` as a dev-dependency (the
//! route-role installer that builds the real router, the demo seed, and
//! `api_router`). These leave with the router-assembly lift and the CLI owners,
//! not with another repoint.
//!
//! Add an item here only if it is platform runtime that cannot live lower yet.
//! If it is someone's domain, lower it instead: a surface that needs another
//! context reaches it through a domain crate below `oxy-app` (rule S3), and
//! `tests/routing/surface_contract.rs` fails on any other `oxy_app::` path.

/// The fence that keeps a sandbox agent token (`oxy_sbx_`) to the one shape it
/// may reach on the `/customer-apps` serve tree. Asked before authentication by
/// the serve route, which sits above this crate. Platform authz, not a tenant
/// context: it reads the presented credential's prefix and nothing else.
pub use crate::server::api::middlewares::app_grant_scope::serve_tree_refuses;

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

/// What the integration tests of the surface crates drive: the served router,
/// which the custom-app tests exercise in a staging or sandbox environment, the
/// external auth stack, and the publish-token scope middleware. Tests only; no
/// surface's `src` takes anything from here.
pub mod testing {
    pub use crate::server::api::middlewares::app_publish_token_scope;
    pub use crate::server::router::{api_router, external_auth_layers, flat_api_surface};

    /// The router module itself, for the items tests name beside the ones above.
    pub use crate::server::router;
}

/// Route-role (pod placement) declarations, as a test installs them: the real
/// router's declarations plus the ones a surface crate hands in, so `classify`
/// answers for the surface's routes too. Kept in `oxy-app` because it builds
/// the router; the production half is `oxy_app_core::pods::role_manifest`.
pub mod roles {
    pub use crate::server::role_manifest::install_route_declarations_for_tests_with;
}
