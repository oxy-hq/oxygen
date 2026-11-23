//! What is left of the project-scoped endpoints custom-app bundles call.
//!
//! The routes themselves all live in `oxy-api-custom-apps` now (the query data
//! plane, the metric-tree twins, the agent ask, the agent-run stream, the
//! automation run) and `oxy-api-world-model` (the world-model twins). What
//! stays here is the semantic boundary's old path, which `tests/routing`
//! reaches. (The workspace-actions mode read, `PlanesWorkspaceActions`, went
//! down to `oxy_custom_apps::workspace_actions` on 2026-11-16, the run row's
//! state machine to `oxy_custom_apps::automation_run` on 2026-11-22, and the
//! semantic boundary to `oxy_workspace_runtime::semantic_boundary` the same
//! day. The run row's re-exports, `automation_run`, which only the prelude
//! read, were deleted in decompose R1.)

/// Down to `oxy-workspace-runtime` on 2026-11-22 (W6a); the old path stays so
/// no call site changes.
pub use oxy_workspace_runtime::semantic_boundary;
