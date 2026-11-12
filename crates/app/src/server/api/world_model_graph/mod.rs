//! World Model graph endpoints for the IDE:
//!
//! The entity-centric world model (every primary entity in the semantic
//! model, its measures, and promotion edges) plus instance drill-down
//! (instance picker, filter counts, instance detail, measure breakdown).
//!
//! Split out of `semantic.rs` (file-size guideline): these handlers share
//! the semantic-model load + query-execution path with the semantic
//! endpoints but form a self-contained surface. Distinct from
//! `world_model.rs`, which serves the live world-model *app* (cameras,
//! weather, event SSE, LLM proxy).
//!
//! The cores both this surface and the customer-app twins answer from — the
//! `Wm*` DTOs, the entity graph, the instance listing, the measure breakdown
//! and the SQL/traversal helpers — are `oxy_world_model::graph`, re-exported
//! here. What stays:
//! - [`source`]  — where the model is read from (compile boundary first), and
//!   which databases a pod with no working copy refuses.
//! - [`warehouse`] — the connector the cores run on, as the platform builds it.
//! - [`handlers`] — the six HTTP handlers.

mod handlers;
mod source;
mod warehouse;

pub use handlers::{
    get_world_model, get_world_model_filter_instances, get_world_model_instance_detail,
    get_world_model_instances, get_world_model_measure_breakdown, post_world_model_filter_counts,
};
pub use oxy_world_model::graph::types::*;

// Transport-agnostic reuse cores shared with the customer-app-gated
// `projects/world_model.rs` handlers and the metric-tree ops.
pub(crate) use oxy_world_model::graph::{
    build_world_model_response, instance_scope_filters, instances_core, instances_view,
    measure_breakdown_core,
};
pub(crate) use warehouse::WorkspaceWarehouse;
