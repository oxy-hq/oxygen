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

/// Boot-time refusals: a process environment, encryption key or auth provider
/// that cannot serve the role it was started in.
pub mod shared_state_check {
    pub use crate::server::shared_state_check::{
        check_encryption_key, check_process_env, load_master_key,
    };
}

pub mod auth_provider_check {
    pub use crate::server::auth_provider_check::{check, provider_configured};
}

/// The feature-flag cache. `serve` and `worker` load it before any request or
/// task reads a flag.
pub mod feature_flags {
    pub use crate::server::feature_flags::cache::init;
}

/// The router `serve` mounts, and the seams `oxy-server` composes into it.
pub mod router {
    pub use crate::server::router::{
        GlobalTickSeam, NotificationsSeam, SurfaceSeams, TaskSeam, api_router, build_cors_layer,
        build_openapi_doc, install_global_ticks, internal_api_router, require_notifications,
        require_task_kinds,
    };

    /// The run drivers: the startup recovery pass, the periodic one, and the
    /// shutdown hooks `serve` waits on. `OXY_INPROC_GLOBAL_WORKER` is the kill
    /// switch that turns driving off on a worker.
    pub mod recovery {
        pub use crate::server::router::recovery::{
            INPROC_GLOBAL_WORKER_ENV, StartupPass, await_shutdown_hooks,
            inproc_global_worker_enabled, spawn_recovery,
        };
    }
}

/// The layers `serve` wraps the router in.
pub mod middleware {
    pub use crate::server::admission::admission_control;
    pub use crate::server::api::middlewares::request_id::{
        REQUEST_ID_HEADER, request_id_middleware,
    };
    pub use crate::server::api::middlewares::sentry_surface::tag_custom_app_surface;
    pub use crate::server::api::projects::workspace_actions::PlanesWorkspaceActions;
    pub use crate::server::role_middleware::{enforce_role, enforce_role_external};
}

/// The `ETag` helpers the SPA handler answers conditional requests with.
pub mod http_cache {
    pub use crate::server::http_cache::{if_none_match, weak_etag};
}

/// The Prometheus listener (`OXY_METRICS_PORT`).
pub mod metrics_server {
    pub use crate::server::metrics_server::{resolve_metrics_port, start};
}

/// Dev sign-in, which `serve` announces at boot when it is on.
pub mod dev_login {
    pub use crate::server::api::auth::{
        dev_login_is_loopback_only, dev_login_source, is_dev_login_enabled,
    };
}

/// The standalone worker's runtime, health listener and metrics.
pub mod worker {
    pub use crate::server::api::middlewares::workspace_context::PreaggCacheCtx;
    pub use crate::server::worker_health::{self, WorkerHealthState};
    pub use crate::server::worker_metrics::{ConcurrencyCaps, MetricsState};
    pub use crate::server::worker_runtime::WorkerRuntime;
}

/// The custom-app bundle cache's byte budget, sized at boot.
pub use crate::server::api::custom_apps_bundle_cache::resolve_budget as bundle_cache_budget;

/// The Slack integration's background work: Socket Mode and the row sweeps.
pub use crate::integrations::slack::boot::start_background as start_slack_background;
