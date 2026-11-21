//! HTTP server and API endpoints

pub mod admission;
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
pub mod compile_reconcile;
pub mod compile_request;
pub mod compile_worker;
pub use oxy_app_core::default_branch;
pub mod factory_access;
// `feature_flags` (the flag routes: the staff console's list, patch and
// evaluate, and the org member's read) is `oxy-api-admin`'s since 2026-11-21;
// the runtime is `oxy-feature-flags`.
pub mod http_cache;
pub mod ide_proxy;
pub mod metrics_server;
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
pub mod runtime_artifact;
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
