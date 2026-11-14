//! What is left of the project-scoped endpoints custom-app bundles call.
//!
//! The routes themselves all live in `oxy-api-custom-apps` now (the query data
//! plane, the metric-tree twins, the agent ask, the agent-run stream, the
//! automation run) and `oxy-api-world-model` (the world-model twins). What
//! stays here is the shared platform half they reach through
//! `oxy_app::surface`: the semantic boundary, the automation run's row state
//! machine, and the workspace-actions mode read.

pub mod automation_run;
pub mod semantic_boundary;
pub mod workspace_actions;
