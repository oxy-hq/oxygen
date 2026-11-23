//! HTTP server and API endpoints

pub mod api;
pub mod audit_anchor;
pub mod auth_provider_check;
pub mod authz;
pub mod build_identity;
// The compile boundary's runtime leaves, `oxy-compile-runtime`'s since
// 2026-11-21; re-exported so every caller and `tests/platform` keep these
// paths.
pub use oxy_compile_runtime::config_gate as compile_config_gate;
pub use oxy_compile_runtime::git as compile_git;
pub use oxy_compile_runtime::maintenance as compile_maintenance;
pub use oxy_compile_runtime::oltp as compile_oltp;
// The branch check, the push webhook's reconciler, the env pins and the
// automatic compile enqueue, `oxy-compile-runtime`'s `reconcile` since
// 2026-11-22; re-exported so the router, the workspace middleware, the compile
// worker, `oxy-server` and `tests/platform` keep this path.
pub use oxy_compile_runtime::reconcile as compile_reconcile;
// The branch compile and its status read, `oxy-compile-runtime`'s `request`
// since 2026-11-22; re-exported so `tests/platform` keeps this path.
pub use oxy_compile_runtime::request as compile_request;
// The compile task's worker, `oxy-compile-runtime`'s `worker` since 2026-11-22
// (decompose C4); re-exported so the compile dispatcher and the periodic
// reconcile keep this path.
pub use oxy_app_core::default_branch;
pub use oxy_compile_runtime::worker as compile_worker;
// The Factory-access gate went down into `oxy-workspace-runtime::request` beside
// its one caller, the workspace middleware (decompose W7, 2026-11-22).
pub use oxy_workspace_runtime::request::factory_access;
// `feature_flags` (the flag routes: the staff console's list, patch and
// evaluate, and the org member's read) is `oxy-api-admin`'s since 2026-11-21;
// the runtime is `oxy-feature-flags`.
pub mod http_cache;
// Pod placement (the Factory relay, the role middleware, servability and the
// route roles) is `oxy-app-core`'s `pods` since 2026-11-22 (decompose G3);
// re-exported so the router and `host` keep these paths.
// `role_manifest` and `role_middleware` stay files here: they hold the test
// installer that builds the real router, and the tests that run on it.
pub use oxy_app_core::pods::ide_proxy;
pub use oxy_app_core::preagg_context;
// The preagg cycle's task executor, `oxy-workspace-runtime`'s since
// 2026-11-22 (W4): its `CycleWorkspace` impl for `OxyProjectContext` has to
// live beside that type (the orphan rule). Re-exported so the recovery
// path's registration keeps this path.
pub use oxy_workspace_runtime::preagg_executor;
// The compile worker's post-promote nudge, `oxy-preagg`'s `promote` since
// 2026-11-20; re-exported so `tests/platform` keeps this path.
pub use oxy_preagg::promote as preagg_promote;
pub mod previews;
// The claim-safe attempt every queued analytics run's executor shares, the
// chat executor and who a driven run runs as, `oxy-workspace-runtime`'s since
// decompose W6b (2026-11-23); re-exported so recovery, `periodic` and
// `tests/platform` keep this path.
pub use oxy_workspace_runtime::queued_analytics;
pub mod role_manifest;
pub mod role_middleware;
pub mod route_catalog;
pub mod router;
// The task router's LISTEN settings, `oxy-app-core`'s `listener` since 2026-11-22.
pub use oxy_app_core::listener as task_router_listener;
#[cfg(test)]
pub(crate) mod test_support;
mod token_usage_flush;
pub mod workspace_fs;
pub use oxy_app_core::workspace_repo_facts;

pub use router::{AppState, WorkspaceExtractor, api_router, openapi_router};
