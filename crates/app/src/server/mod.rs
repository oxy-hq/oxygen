//! HTTP server and API endpoints
//!
//! Only re-exports of lower crates are left here, each kept because the CLI,
//! `host`, `oxy-server` or `oxy-app`'s own `tests/` still names its old path.
//! Decompose SHM (2026-11-23) deleted the ones nothing named any more
//! (`compile_maintenance`, `compile_worker`, `factory_access`, `ide_proxy`,
//! `preagg_context`, `preagg_executor`, `task_router_listener`, `workspace_fs`):
//! import the crate that holds the code.

pub mod api;
pub mod authz;
pub mod build_identity;
// The compile boundary's runtime leaves, `oxy-compile-runtime`'s since
// 2026-11-21; re-exported so the CLI and `tests/platform` keep these paths.
pub use oxy_compile_runtime::config_gate as compile_config_gate;
pub use oxy_compile_runtime::git as compile_git;
pub use oxy_compile_runtime::oltp as compile_oltp;
// The branch check, the push webhook's reconciler, the env pins and the
// automatic compile enqueue, `oxy-compile-runtime`'s `reconcile` since
// 2026-11-22; re-exported so `oxy-server` and `tests/platform` keep this path.
pub use oxy_compile_runtime::reconcile as compile_reconcile;
// The branch compile and its status read, `oxy-compile-runtime`'s `request`
// since 2026-11-22; re-exported so `tests/platform` keeps this path.
pub use oxy_compile_runtime::request as compile_request;

// The default-branch resolver and the repository facts, `oxy-app-core`'s;
// `tests/platform` keeps these paths.
pub use oxy_app_core::{default_branch, workspace_repo_facts};

// `host::http_cache` re-exports this for `oxy-server`'s SPA handler.
pub mod http_cache;
// The compile worker's post-promote nudge, `oxy-preagg`'s `promote` since
// 2026-11-20; re-exported so `tests/platform` keeps this path.
pub use oxy_preagg::promote as preagg_promote;
pub mod previews;
// The claim-safe attempt every queued analytics run's executor shares, the
// chat executor and who a driven run runs as, `oxy-workspace-runtime`'s since
// decompose W6b (2026-11-23); re-exported so `tests/platform` keeps this path.
pub use oxy_workspace_runtime::queued_analytics;
