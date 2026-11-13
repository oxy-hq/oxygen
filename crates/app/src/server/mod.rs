//! HTTP server and API endpoints

pub mod admission;
pub mod anomaly_notify;
pub mod api;
pub mod audit_anchor;
pub mod auth_provider_check;
pub mod authz;
pub mod build_identity;
pub mod builder_test_runner;
pub mod compile_config_gate;
pub mod compile_git;
pub mod compile_maintenance;
pub mod compile_oltp;
pub mod compile_reconcile;
pub mod compile_request;
pub mod compile_trigger;
pub mod compile_worker;
pub use oxy_app_core::default_branch;
pub mod factory_access;
pub mod feature_flags;
pub mod http_cache;
pub mod ide_proxy;
pub mod metrics_server;
pub use oxy_app_core::preagg_context;
pub mod preagg_executor;
mod preagg_freshness;
mod preagg_generation;
mod preagg_ledger;
pub mod preagg_promote;
pub(super) mod preagg_rebuild;
mod preagg_retract;
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
// The harness's own tests sit beside it rather than inside it, so the surface
// crates that `#[path]`-include `test_support.rs` get the harness alone.
#[cfg(test)]
#[path = "test_support_tests.rs"]
mod test_support_tests;
mod token_usage_flush;
pub mod worker_health;
pub mod worker_metrics;
pub mod worker_runtime;
pub mod workspace_fs;
pub use oxy_app_core::workspace_repo_facts;

pub use router::{AppState, WorkspaceExtractor, api_router, openapi_router};
