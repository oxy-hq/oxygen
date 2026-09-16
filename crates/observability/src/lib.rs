//! Observability crate for Oxy.
//!
//! Provides the `ObservabilityStore` trait with its sole backend (ClickHouse),
//! a tracing `SpanCollectorLayer`, and the `init_observability` bridge that
//! wires them together.

pub mod backends;
pub mod burn_rate;
pub mod custom_app_sink;
pub mod duration;
pub mod fleet_health;
pub(crate) mod flush_queue;
pub mod global;
pub mod heartbeat;
pub mod intent_types;
pub mod layer;
pub mod store;
pub mod telemetry;
pub mod types;

pub use burn_rate::{
    ALERT_WINDOWS_MINUTES, BurnVerdict, Severity, SloConfig, evaluate as evaluate_burn_rate,
};
pub use custom_app_sink::{
    record_client_errors as record_custom_app_client_errors,
    record_event as record_custom_app_event, record_logs as record_custom_app_logs,
    spawn_custom_app_bridges,
};
pub use duration::{DURATIONS, DurationWindow, RETENTION_DAYS};
pub use global::{get_global, set_global};
pub use layer::{SpanCollectorLayer, current_trace_id};
pub use store::ObservabilityStore;
pub use telemetry::{
    build_layer_and_receiver, build_observability_layer, init_observability, init_stdout,
    observability_filter, shutdown, spawn_bridge,
};
pub use types::*;
