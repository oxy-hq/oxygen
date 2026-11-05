//! Backend feature flag system: global switches, per-org targeting and A/B splits.
//!
//! - `registry` — code-declared list of flags, their variants and defaults.
//! - `targeting` — an admin's rules (overrides, split) and their validation.
//! - `evaluate` — pure evaluation: flag state + [`FlagContext`] → variant + reason.
//! - `store` — Sea-ORM helpers for the `feature_flags` table.
//! - `cache` — in-memory cache, preloaded at startup, write-through on
//!   admin PATCH. Exposes `is_enabled(key)` for read-site gating.
//! - `exposure` — split assignments logged for experiment analysis.
//! - `routes` — admin API (list, patch, evaluate) and the org-member read.
//!
//! Reading a flag: [`is_enabled`] for a global switch, [`is_enabled_for`] or
//! [`variant`] with the org (and person) for anything targeted.
//! `internal-docs/feature-flags.md` is the guide.

pub mod cache;
pub mod evaluate;
pub mod exposure;
pub mod registry;
pub mod routes;
pub mod store;
pub mod targeting;

pub use cache::is_enabled;
pub use evaluate::{Evaluation, FlagContext, Reason};

/// The variant `key` serves `ctx`, recording a split assignment as an exposure.
/// A key the registry does not declare reads as off, with [`Reason::Error`].
pub fn evaluate_for(key: &'static str, ctx: &FlagContext) -> Evaluation {
    let (Some(def), Some(eval)) = (registry::get(key), cache::evaluate(key, ctx)) else {
        return Evaluation {
            variant: registry::BOOLEAN[0],
            reason: Reason::Error,
            bucket: None,
        };
    };
    exposure::record(def, &eval, ctx);
    eval
}

/// The variant name `key` serves `ctx`: `"control"`, `"treatment"`, `"on"`…
pub fn variant(key: &'static str, ctx: &FlagContext) -> &'static str {
    evaluate_for(key, ctx).variant
}

/// Whether `key` serves `ctx` anything but its off/control variant.
pub fn is_enabled_for(key: &'static str, ctx: &FlagContext) -> bool {
    let eval = evaluate_for(key, ctx);
    registry::get(key).is_some_and(|def| eval.variant != def.off())
}

/// The apps-first experience, in one switch: the customer home and onboarding, the
/// customer's read-only world model page, the web app without its workspace
/// management and creation screens, and Factory access enforcement
/// (`server::factory_access`). Targeted per org: every read site passes the org,
/// so an org override or a rollout split decides it for that org.
pub const APPS_FIRST_EXPERIENCE: &str = "apps_first_experience";

/// Whether [`APPS_FIRST_EXPERIENCE`] is on for `org_id`.
pub fn apps_first_experience_in(org_id: uuid::Uuid) -> bool {
    is_enabled_for(APPS_FIRST_EXPERIENCE, &FlagContext::org(org_id))
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
