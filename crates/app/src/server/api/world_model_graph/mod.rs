//! The World Model Graph's execution seam, as the platform builds it.
//!
//! The IDE handlers are `oxy-api-world-model`'s (`graph`), and the cores they
//! and the customer-app twin (`projects::world_model`) answer from are
//! `oxy_world_model::graph`. What stays here is [`WorkspaceWarehouse`]: the
//! `GraphWarehouse` those cores run on, built on the connector every Dev
//! Portal query uses. It stays because the twin needs it too, and the twin
//! waits on the custom-app gate split; the IDE surface takes it through
//! `oxy_app::surface::world_model_exec`.

mod warehouse;

pub use warehouse::WorkspaceWarehouse;
