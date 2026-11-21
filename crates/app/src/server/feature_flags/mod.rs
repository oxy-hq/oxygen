//! Backend feature flags: the HTTP half. The runtime (registry, targeting,
//! evaluation, store, cache, exposures, rollout gates) is `oxy-feature-flags`,
//! below `oxy-app`, which every caller imports directly.
//!
//! - `routes` — admin API (list, patch, evaluate) and the org-member read.
//!
//! `internal-docs/feature-flags.md` is the guide.

pub mod routes;
