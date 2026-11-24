//! What is left of `server::api`: re-exports of lower crates that `oxy-app`'s
//! own `tests/` still name at these paths.
//!
//! Every HTTP surface that lived here is its own `api-<context>` crate now,
//! mounted by `oxy-server` (`internal-docs/domain-boundaries.md`). Decompose
//! SHM (2026-11-23) deleted the re-exports nothing named any more (`auth`,
//! `custom_apps_agent_body`, the `custom_apps_*` paths of `oxy-custom-apps`
//! other than the bundle cache, `metric_tree_groups`, `metric_tree_inputs` and
//! `workspace_org`), and decompose RPT (2026-11-24) the bundle cache's: import
//! the crate that holds the code.

pub mod billing;
pub use oxy_app_core::compiled_reader;
/// Lowered into the domain crate; `tests/platform` keeps the old path.
pub use oxy_metric_monitoring::metric_tree_baseline;
/// The data plane's gate and project-context composition root, down to
/// `oxy-workspace-runtime` on 2026-11-22 (W6a); `tests/platform` keeps the old
/// path.
pub use oxy_workspace_runtime::custom_apps_gates;
pub mod middlewares;
pub mod projects;
