//! Shared semantic-query cores for the World Model Graph surfaces.
//!
//! This crate holds query/compute logic shared by `oxy-app`'s world-model and
//! metric-tree HTTP surfaces — and, once it is extracted, the `world_model_graph`
//! sibling crate. Today it hosts `.world-model.yml` config parsing; the graph
//! assembly / instance-listing / measure-breakdown cores and their
//! `QueryExecutor` port land here as they are lowered out of `oxy-app`.
//!
//! It sits below `oxy-app` and above `oxy` / `oxy-semantic`, so the surfaces can
//! share these cores without a crate cycle.

mod world_model_config;

pub use world_model_config::{
    WmEntityConfig, WmFieldConfig, WorldModelConfig, WorldModelConfigError,
};
