//! What `oxy-server`'s long-lived commands (`serve`, `start`, `worker`) take
//! from `oxy-app`: the process checks they run at boot, the router they mount,
//! the migrations they run first, and the run drivers and worker runtime.
//!
//! The counterpart of [`crate::surface`]. Like it, this module holds no code,
//! only re-exports, which is why `tests/routing/app_growth.rs` admits it to
//! the closed top level: it adds no context and shrinks as `oxy-app` does. That module is what an HTTP surface
//! crate (`api-*`) may import; this one is what the binary that hosts the
//! process may. The commands moved up out of `oxy-app`
//! (`internal-docs/domain-boundaries.md`, "End state: no `oxy-app`", step b),
//! and everything they still need from here is listed below, so this file is
//! also the inventory of what the base crate (step c) has to carry.
//!
//! Grouped by the module each item lives in, so a call site reads
//! `host::role_manifest::current_process_role()` and still says where the item
//! is defined.

/// Database migrations: every domain's migrator under one advisory lock.
/// `oxy migrate` runs the same function.
pub mod migrations {
    pub use crate::cli::commands::migrate::{run_database_migrations, skip_migrations_requested};
}

/// Which fleet role this process plays, and the route table's role declarations.
pub mod role_manifest {
    pub use crate::server::role_manifest::*;
}

/// Boot-time refusals: an auth provider that cannot serve the role it was
/// started in. (The process-environment and encryption-key checks,
/// `shared_state_check`, moved up into `oxy-server` on 2026-11-22.)
pub mod auth_provider_check {
    pub use crate::server::auth_provider_check::{check, provider_configured};
}

/// Which build this process is: the image tag and the build information
/// `oxy-app`'s build script stamps (`GIT_HASH`, `BUILD_TIMESTAMP`, the
/// `GITHUB_*` run). The health probes (`/api/version`, `/api/health`) and
/// route discovery (`/api/_catalog`'s `build`) report it, from `oxy-server`
/// since 2026-11-16; `oxy migrate`'s report reads it here too.
pub mod build_identity {
    pub use crate::server::build_identity::{BuildInfo, build_info, build_label, image_tag};
}

/// The route table's types, as `GET /api/_catalog` (`oxy-server`'s since
/// 2026-11-16) answers with them: the table `oxy-route-catalog` generates, one
/// route as the response describes it, and the fields `?filter=` matches.
pub mod route_catalog {
    pub use crate::server::route_catalog::{RouteCatalog, RouteDescription, searchable_fields};
}

/// The router `serve` mounts, and the seams `oxy-server` composes into it.
pub mod router {
    pub use crate::server::router::{
        ApiRouterArgs, FleetJobDeps, FleetJobSeam, GlobalTickSeam, NotificationsSeam, SurfaceSeams,
        TaskSeam, TickPass, api_router, api_router_with_state, build_cors_layer, build_openapi_doc,
        install_global_ticks, internal_api_router, require_fleet_jobs, require_notifications,
        require_task_kinds, start_fleet_jobs,
    };

    /// The run drivers: the startup recovery pass, the periodic one, and the
    /// shutdown hooks `serve` spawns, registers and waits on.
    /// `OXY_INPROC_GLOBAL_WORKER` is the kill switch that turns driving off on a
    /// worker. `spawn_shutdown_hook` and `register_shutdown_hook` are here for
    /// `serve`'s boot (decompose BL-W) until the drivers move up (BL1).
    pub mod recovery {
        pub use crate::server::router::recovery::{
            INPROC_GLOBAL_WORKER_ENV, StartupPass, await_shutdown_hooks,
            inproc_global_worker_enabled, register_shutdown_hook, spawn_recovery,
            spawn_shutdown_hook,
        };
    }
}

/// The per-token usage flush `serve`'s boot starts (decompose BL-W), until it
/// moves up with the drivers (BL1).
pub mod token_usage_flush {
    pub use crate::server::token_usage_flush::spawn_token_usage_flush;
}

/// The layers `serve` wraps the router in.
pub mod middleware {
    pub use crate::server::api::middlewares::request_id::{
        REQUEST_ID_HEADER, request_id_middleware,
    };
    pub use crate::server::api::middlewares::sentry_surface::tag_custom_app_surface;
    pub use crate::server::role_middleware::{enforce_role, enforce_role_external};
}

/// The `ETag` helpers the SPA handler answers conditional requests with.
pub mod http_cache {
    pub use crate::server::http_cache::{if_none_match, weak_etag};
}

/// What the standalone worker builds its pre-aggregation context from. Its
/// runtime, health listener and metrics moved up into `oxy-server` on
/// 2026-11-22 (decompose UP2).
pub mod worker {
    pub use crate::server::api::middlewares::workspace_context::PreaggCacheCtx;
}

/// The periodic driver's tick counters in Prometheus text, which the worker's
/// `GET /metrics` (`oxy-server`'s `worker_metrics`) appends. The tick that
/// increments them stays here.
pub mod periodic_metrics {
    pub use crate::server::router::periodic::metrics::push_prometheus;
}

/// The custom-app bundle cache's byte budget, sized at boot.
pub use crate::server::api::custom_apps_bundle_cache::resolve_budget as bundle_cache_budget;
