//! Shared seam between the `oxy-app` HTTP layer and the crates split out of it.
//!
//! Everything here was previously a module of `oxy-app`. It lives in its own
//! crate so the per-surface crates (admin, integrations, custom apps, …) can
//! depend on the pieces they share *without* depending on each other — that is
//! what lets their frontends compile in parallel instead of serially inside one
//! 140k-line crate.
//!
//! Nothing in here may depend on a per-surface crate. If something in this crate
//! needs a surface, the dependency is pointing the wrong way.

pub mod app_state;
pub mod audit;
pub mod custom_apps_host_dispatch;
pub mod member_authz;
pub mod org_host_dispatch;
pub mod pagination;
pub mod serve_mode;
pub mod workspace_cache;

pub use app_state::AppState;
