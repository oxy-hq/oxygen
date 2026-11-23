//! Backend feature flag system: global switches, per-org targeting and A/B splits.
//!
//! The runtime's domain half, below `oxy-app` and every `api-*` surface: no
//! routes, no `AppState`. Every reader imports it directly: `oxy-app`, the
//! `api-*` surfaces and the `oxy` binary. The HTTP half
//! (`oxy_api_admin::feature_flags`: the staff console's list, patch and
//! evaluate, and the org member's read) is `oxy-api-admin`'s since 2026-11-21.
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

/// App events, per org: an Oxy Function's `ctx.events.publish` and
/// `tx.events.publish` put an event in the org's outbox only where this serves
/// `on` (app events design §10, step 2). Read with [`is_enabled_for`] and the
/// publishing app's org; everywhere else a publish is refused `EventsNotEnabled`.
pub const APP_EVENTS: &str = "app_events";

/// The react analytics pilot, per org, and its only switch: where this serves
/// `on`, a served analytics chat runs on the react runtime (an agent react
/// cannot serve, with no read-only database or a key react does not honor,
/// stays on the FSM). An agent's `runtime:` key is read by `oxy test` alone.
/// Read with [`is_enabled_for`] and the workspace's org
/// (`OxyProjectContext::react_runtime_enabled`); off, every chat answers on the
/// FSM, with no pull request needed.
pub const REACT_RUNTIME: &str = "react_runtime";

/// Per-user memory for react runs, per org: a served react run reads the
/// requesting user's notes for the workspace and may save new ones in a
/// reflection turn, only where this serves `on` (and [`REACT_RUNTIME`] put
/// the run on react). Read with [`is_enabled_for`] and the workspace's org
/// (`OxyProjectContext::react_memory_enabled`).
pub const REACT_MEMORY: &str = "react_memory";
