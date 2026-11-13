//! World Model Graph endpoints for the IDE: the entity-centric world model
//! (every primary entity in the semantic model, its measures, and promotion
//! edges) plus instance drill-down (instance picker, filter counts, instance
//! detail, measure breakdown).
//!
//! Moved here from `oxy-app`'s `server::api::world_model_graph`. The cores
//! these handlers and the customer-app twin (`oxy-app`'s
//! `projects::world_model`) answer from — the `Wm*` DTOs, the entity graph,
//! the instance listing, the measure breakdown and the SQL/traversal helpers —
//! are `oxy_world_model::graph`. The warehouse they run on is the platform's
//! (`oxy_app::surface::world_model_exec`). What is here:
//! - `source` — where the model is read from (compile boundary first), and
//!   which databases a pod with no working copy refuses.
//! - `handlers` — the six HTTP handlers.

mod handlers;
mod source;

pub use handlers::{
    get_world_model, get_world_model_filter_instances, get_world_model_instance_detail,
    get_world_model_instances, get_world_model_measure_breakdown, post_world_model_filter_counts,
};
