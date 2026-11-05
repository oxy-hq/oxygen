//! Backend feature flag system.
//!
//! - `registry` — code-declared list of flags + defaults.
//! - `store` — Sea-ORM helpers for the `feature_flags` table.
//! - `cache` — in-memory cache, preloaded at startup, write-through on
//!   admin PATCH. Exposes `is_enabled(key)` for read-site gating.
//! - `routes` — admin API: list + patch.

pub mod cache;
pub mod registry;
pub mod routes;
pub mod store;

pub use cache::is_enabled;

/// The apps-first experience, in one switch: the customer home and onboarding, the
/// customer's read-only world model page, the web app without its workspace
/// management and creation screens, and Factory access enforcement
/// (`server::factory_access`). Global: the registry has no per-org targeting.
pub const APPS_FIRST_EXPERIENCE: &str = "apps_first_experience";

/// Whether [`APPS_FIRST_EXPERIENCE`] is on.
pub fn apps_first_experience() -> bool {
    is_enabled(APPS_FIRST_EXPERIENCE)
}

/// The third home, on top of [`APPS_FIRST_EXPERIENCE`]: the ask box first, the apps
/// big, Today folded to one line, two views (simple and full) and a top bar with
/// no clock and a Today button. Off, the apps-first home is the one before it.
/// Presentation only: the web app reads it from `GET /auth/config`.
pub const HOME_V3: &str = "home_v3";

/// Whether [`HOME_V3`] is on.
pub fn home_v3() -> bool {
    is_enabled(HOME_V3)
}
