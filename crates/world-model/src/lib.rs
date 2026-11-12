//! The world-model context's domain half, below both `oxy-app` and
//! `oxy-api-world-model`.
//!
//! What lives here:
//!
//! - `.world-model.yml` config parsing, read by `oxy-app`'s World Model Graph
//!   and metric-tree surfaces.
//! - [`graph`], the World Model Graph's cores: the entity graph, the instance
//!   listing and the measure breakdown, with the SQL and traversal helpers
//!   behind them. The IDE routes and their customer-app twins both answer
//!   from them; the warehouse they run on is a seam `oxy-app` implements.
//! - [`cohort`], the peer-cohort wire contract and pure helpers both cohort
//!   surfaces share.
//! - [`live_feed`], the live feed's write half: the events and the outbox that
//!   appends them to `world_model_events`. `oxy-app` publishes (the Toast
//!   webhook, the UniFi watcher); `oxy-api-world-model` publishes too (the
//!   camera-domain bridge) and owns the read half.
//!
//! It sits above `oxy` / `entity`, so the publishers on both sides share the
//! feed without either depending on the other.

pub mod cohort;
pub mod graph;
pub mod live_feed;
mod world_model_config;

pub use world_model_config::{
    WmEntityConfig, WmFieldConfig, WorldModelConfig, WorldModelConfigError,
};
