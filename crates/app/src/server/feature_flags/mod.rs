//! Backend feature flags: the runtime's domain half re-exported from
//! `oxy-feature-flags` (registry, targeting, evaluation, store, cache,
//! exposures, rollout gates), so every caller keeps its
//! `server::feature_flags::*` path, beside the HTTP half that stays here.
//!
//! - `routes` — admin API (list, patch, evaluate) and the org-member read.
//!
//! `internal-docs/feature-flags.md` is the guide.

pub use oxy_feature_flags::*;

pub mod routes;
