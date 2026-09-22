//! HTTP server and API endpoints

pub mod admission;
pub mod api;
pub mod app_function_executor;
pub mod audit_anchor;
pub mod authz;
pub mod builder_test_runner;
pub mod compile_config_gate;
pub mod compile_maintenance;
pub mod compile_oltp;
pub mod compile_trigger;
pub mod compile_worker;
pub mod default_branch;
pub mod feature_flags;
pub mod git_fetch_maintenance;
pub mod health_eval_executor;
pub mod http_cache;
pub mod ide_proxy;
pub mod metrics_server;
pub mod preagg_context;
pub mod preagg_executor;
mod preagg_freshness;
mod preagg_generation;
mod preagg_ledger;
pub(super) mod preagg_rebuild;
mod preagg_retract;
mod preagg_workspace;
pub mod role_manifest;
pub mod role_middleware;
pub mod route_catalog;
pub mod router;
pub mod runtime_artifact;
pub mod serve_safety;
pub mod service;
pub mod simulation;
#[cfg(test)]
pub(crate) mod test_support;
pub mod worker_health;
pub mod worker_metrics;
pub mod worker_runtime;
pub mod workspace_fs;
pub mod worktree_registry;

pub use router::{AppState, WorkspaceExtractor, api_router, openapi_router};
