//! Backend feature flag system: global switches, per-org targeting and A/B splits.
//!
//! The runtime's domain half, below `oxy-app` and every `api-*` surface: no
//! routes, no `AppState`. `oxy-app` re-exports it at `server::feature_flags`,
//! beside the HTTP half (`server::feature_flags::routes`: the staff console's
//! list, patch and evaluate, and the org member's read), which stays there
//! until it moves to its owning surface (`internal-docs/domain-boundaries.md`,
//! Pending).
//!
//! - `registry` — code-declared list of flags, their variants and defaults.
//! - `targeting` — an admin's rules (overrides, split) and their validation.
//! - `evaluate` — pure evaluation: flag state + [`FlagContext`] → variant + reason.
//! - `store` — Sea-ORM helpers for the `feature_flags` table.
//! - `cache` — in-memory cache, preloaded at startup, write-through on
//!   admin PATCH. Exposes `is_enabled(key)` for read-site gating.
//! - `exposure` — split assignments logged for experiment analysis.
//! - `rollout` — rollout gates: `Rollout` flags with a sunset and, for one
//!   release, the environment variable that used to be the gate.
//!
//! Reading a flag: [`is_enabled`] for a global switch, [`is_enabled_for`] or
//! [`variant`] with the org (and person) for anything targeted.
//! `internal-docs/feature-flags.md` is the guide.

pub mod cache;
pub mod evaluate;
pub mod exposure;
pub mod registry;
pub mod rollout;
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

/// The third home: the ask box first, the apps
/// big, Today folded to one line, two views (simple and full) and a top bar with
/// no clock and a Today button. Off, the home is the one before it.
/// Presentation only: the web app reads it from `GET /auth/config`.
pub const HOME_V3: &str = "home_v3";

/// App events, per org: an Oxy Function's `ctx.events.publish` and
/// `tx.events.publish` put an event in the org's outbox only where this serves
/// `on` (app events design §10, step 2). Read with [`is_enabled_for`] and the
/// publishing app's org; everywhere else a publish is refused `EventsNotEnabled`.
pub const APP_EVENTS: &str = "app_events";

/// The react analytics pilot, per org: a served chat of an agent that opts in
/// (`runtime: react`) runs on the react runtime only where this serves `on`.
/// Read with [`is_enabled_for`] and the workspace's org
/// (`OxyProjectContext::react_runtime_enabled`); off, every such agent answers
/// on the FSM.
pub const REACT_RUNTIME: &str = "react_runtime";

/// Whether [`HOME_V3`] is on.
pub fn home_v3() -> bool {
    is_enabled(HOME_V3)
}
