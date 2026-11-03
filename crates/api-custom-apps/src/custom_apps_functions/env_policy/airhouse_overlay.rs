//! The Airhouse plane's modes as the policy sees them
//! (`internal-docs/2026-11-02-custom-app-branches-design.md` §5.4).
//!
//! Writes land in the app schema's sibling, `app_<writer>__<env>`, in every
//! mode; what a mode changes is what `ctx.airhouse.query` reads:
//!
//! - `write-only` (the default, today's behaviour): production's tables, live.
//! - `branch`: a copy-on-write overlay. A table the sibling holds — copied
//!   from production on the environment's first write to it — is read from
//!   the sibling; every other table is read live from production.
//! - `snapshot`: `branch`, with every table the overlay does not hold read at
//!   the environment's pin, a DuckLake point in time no more than
//!   [`snapshot_window`] old.
//!
//! Both overlay modes are for sandboxes only (`PlaneSpec::validate` refuses
//! them on staging, `custom_apps_planes::airhouse`).
//!
//! The overlay always sits on production, never on a parent environment's
//! overlay (§2): its shadow rows are keyed by this environment alone.

use oxy_app_core::custom_app_environment::AppEnvironment;
use uuid::Uuid;

use super::{Decision, Target};
use crate::custom_apps_planes::AirhouseMode;

pub use crate::custom_apps_planes::airhouse::{pin_refusal, snapshot_window};

/// What `ctx.airhouse.query` does under `mode`. `write-only` reads
/// production as it always has; the overlay modes read through the sibling.
pub fn query_decision(mode: AirhouseMode) -> Decision {
    match mode {
        AirhouseMode::WriteOnly => Decision::Allow,
        AirhouseMode::Branch => Decision::Isolate(Target::AirhouseOverlay),
        AirhouseMode::Snapshot => Decision::Isolate(Target::AirhouseSnapshot),
    }
}

/// The overlay's key in the shadow registry (`workspace_preview_tables`).
/// A preview's key is `[a-z0-9_]` only, so one with `:` is never a
/// preview's, and the preview drop never claims these rows.
pub fn overlay_key(app_id: Uuid, environment: &AppEnvironment) -> String {
    format!("env:{app_id}:{}", environment.name())
}

/// The fix a `snapshot` read gives when its pin is older than the window.
pub fn reset_fix(app: &str, environment: &AppEnvironment) -> String {
    format!(
        "re-pin it: oxyc env reset {app} {} --plane airhouse",
        environment.name()
    )
}
