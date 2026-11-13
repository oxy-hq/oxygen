//! The world model's customer-app twins: what a custom-app bundle reads
//! through the SDK (`useWorldModel`, `useWorldModelInstances`,
//! `useMeasureBreakdown`, peer cohorts) at
//! `/api/projects/{project_id}/semantic/{world-model*,cohort}`.
//!
//! The IDE serves the same model on the workspace tree (`crate::graph`,
//! `crate::cohort`), and both answer from `oxy_world_model::{graph, cohort}`.
//! What differs is how a caller gets in and what it may see. A bundle passes
//! the customer-app gate, inline (session cookie or API key), so these routes
//! ride `SurfaceSeams::public` outside the auth stack, as they did in
//! `oxy-app`'s public tree. Every read is scoped to the viewer's reach. The
//! gate, the project context, the compile-boundary scan, the project-scoped
//! result cache and the error envelope are the data plane's
//! (`oxy_app::surface::custom_app_semantic`), shared with the metric-tree
//! twins still in `oxy-app`.

pub mod cohort;
pub mod world_model;
