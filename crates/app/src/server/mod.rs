//! HTTP server and API endpoints

pub mod api;
pub mod audit_anchor;
pub mod auth_provider_check;
pub mod authz;
pub mod build_identity;
// The compile boundary's runtime leaves, `oxy-compile-runtime`'s since
// 2026-11-21; re-exported so every caller, the prelude and `tests/platform`
// keep these paths.
pub use oxy_compile_runtime::config_gate as compile_config_gate;
pub use oxy_compile_runtime::git as compile_git;
pub use oxy_compile_runtime::maintenance as compile_maintenance;
pub use oxy_compile_runtime::oltp as compile_oltp;
// The branch check, the push webhook's reconciler, the env pins and the
// automatic compile enqueue, `oxy-compile-runtime`'s `reconcile` since
// 2026-11-22; re-exported so the router, the workspace middleware, the compile
// worker, the prelude, `oxy-server` and `tests/platform` keep this path.
pub use oxy_compile_runtime::reconcile as compile_reconcile;
// The branch compile and its status read, `oxy-compile-runtime`'s `request`
// since 2026-11-22; re-exported so the prelude and `tests/platform` keep this
// path.
pub use oxy_compile_runtime::request as compile_request;
pub mod compile_worker;
pub use oxy_app_core::default_branch;
pub mod factory_access;
// `feature_flags` (the flag routes: the staff console's list, patch and
// evaluate, and the org member's read) is `oxy-api-admin`'s since 2026-11-21;
// the runtime is `oxy-feature-flags`.
pub mod http_cache;
pub mod ide_proxy;
pub use oxy_app_core::preagg_context;
pub mod preagg_executor;
// The compile worker's post-promote nudge, `oxy-preagg`'s `promote` since
// 2026-11-20; re-exported so `compile_worker` and the prelude keep this path.
pub use oxy_preagg::promote as preagg_promote;
pub(crate) mod preagg_workspace;
pub mod previews;
pub mod queued_analytics;
pub mod role_manifest;
pub mod role_middleware;
pub mod route_catalog;
pub mod router;
// The runtime-artifact S3 mirror, `oxy-workspace-runtime`'s since 2026-11-21.
pub use oxy_workspace_runtime::runtime_artifact;
pub mod servability;
pub mod service;
pub mod shared_state_check;
pub mod task_router_listener;
#[cfg(test)]
pub(crate) mod test_support;
mod token_usage_flush;
pub mod worker_health;
pub mod worker_metrics;
pub mod worker_runtime;
pub mod workspace_fs;
pub use oxy_app_core::workspace_repo_facts;

pub use router::{AppState, WorkspaceExtractor, api_router, openapi_router};
