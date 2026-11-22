//! What is left of the project-scoped endpoints custom-app bundles call.
//!
//! The routes themselves all live in `oxy-api-custom-apps` now (the query data
//! plane, the metric-tree twins, the agent ask, the agent-run stream, the
//! automation run) and `oxy-api-world-model` (the world-model twins). What
//! stays here is the shared platform half they reach through
//! `oxy_app::surface`: the semantic boundary and the automation run's preview
//! settle. (The workspace-actions mode read, `PlanesWorkspaceActions`, went
//! down to `oxy_custom_apps::workspace_actions` on 2026-11-16, and the run row's
//! state machine to `oxy_custom_apps::automation_run` on 2026-11-22.)

pub mod automation_run;
pub mod semantic_boundary;
