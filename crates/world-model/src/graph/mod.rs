//! The World Model graph's cores: the entity graph over the semantic model,
//! the instance listing, the measure breakdown, and the SQL and traversal
//! helpers behind them.
//!
//! Two surfaces answer from these and differ only in how a request gets in:
//! the IDE's `/semantic/world-model*` routes, and the customer-app twins that
//! enter through the custom-app gate. They live here, below both, so neither
//! depends on the other. Running a query needs the platform's connector
//! builder, which the cores take as [`GraphWarehouse`]; `oxy-app` implements
//! it. What stays with a surface: where the model is read from, the operating
//! graph's reach and places (the customer-app twin's), and the HTTP shapes.
//!
//! Layout:
//! - [`types`] — the `Wm*` serde DTOs.
//! - [`query`] — SQL and traversal helpers, and their unit tests.
//! - [`assemble`] — the entity graph ([`build_world_model_response`]).
//! - [`instances`] — the instance listing ([`instances_core`]).
//! - [`breakdown`] — the measure breakdown stream ([`measure_breakdown_core`]).
//! - [`warehouse`] — the execution seam.

pub mod assemble;
pub mod breakdown;
pub mod instances;
pub mod query;
pub mod types;
pub mod warehouse;

pub use assemble::build_world_model_response;
pub use breakdown::measure_breakdown_core;
pub use instances::{instances_core, instances_view};
pub use query::{CachedEngine, instance_scope_filters};
pub use types::*;
pub use warehouse::{GraphConnection, GraphWarehouse};

/// A refusal from a core: the status to answer and the message. Each surface
/// wraps it in its own body.
pub type GraphError = (http::StatusCode, String);
