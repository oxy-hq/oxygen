//! The world-model context's domain half, below both `oxy-app` and
//! `oxy-api-world-model`.
//!
//! Two things live here:
//!
//! - `.world-model.yml` config parsing, read by `oxy-app`'s World Model Graph
//!   and metric-tree surfaces. The graph assembly / instance-listing /
//!   measure-breakdown cores land here too as they are lowered out of `oxy-app`.
//! - [`live_feed`], the live feed's write half: the events and the outbox that
//!   appends them to `world_model_events`. `oxy-app` publishes (the Toast
//!   webhook, the UniFi watcher); `oxy-api-world-model` publishes too (the
//!   camera-domain bridge) and owns the read half.
//!
//! It sits above `oxy` / `entity`, so the publishers on both sides share the
//! feed without either depending on the other.

pub mod live_feed;
mod world_model_config;

pub use world_model_config::{
    WmEntityConfig, WmFieldConfig, WorldModelConfig, WorldModelConfigError,
};
