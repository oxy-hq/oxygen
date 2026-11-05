//! Exposure events: the record an A/B analysis joins outcomes to.
//!
//! Only a [`Reason::Split`] assignment is an exposure. A forced variant (an override)
//! or a disabled flag is not a random assignment, and counting one would bias the
//! comparison.
//!
//! Each is a `tracing` event on target [`TARGET`], so it travels with platform
//! telemetry (OTLP → ClickStack) like every other log line; query by that target.
//! Deduplicated per process on `(flag, unit, variant)`, so a hot read site logs a
//! unit once rather than per request. A restart or another pod logs it again: an
//! analysis keeps each unit's FIRST exposure per flag.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use uuid::Uuid;

use super::evaluate::{Evaluation, FlagContext, Reason, unit_id};
use super::registry::FlagDef;

/// The `tracing` target exposure events are logged on.
pub const TARGET: &str = "oxy::feature_flag_exposure";

/// Past this many remembered exposures the set is cleared rather than grown: a
/// duplicate line is cheap, an unbounded set on a long-lived pod is not.
const MAX_REMEMBERED: usize = 100_000;

type Seen = HashSet<(&'static str, Uuid, &'static str)>;

/// Log `eval` as an exposure if it is a split assignment this process has not
/// logged yet.
pub fn record(def: &FlagDef, eval: &Evaluation, ctx: &FlagContext) {
    if eval.reason != Reason::Split {
        return;
    }
    let Some(unit) = unit_id(def, ctx) else {
        return;
    };
    if !first_time(def.key, unit, eval.variant) {
        return;
    }
    tracing::info!(
        target: TARGET,
        flag = def.key,
        variant = eval.variant,
        unit = ?def.unit,
        unit_id = %unit,
        org_id = ctx.org_id.map(|id| id.to_string()),
        user_id = ctx.user_id.map(|id| id.to_string()),
        bucket = eval.bucket,
        "feature flag exposure"
    );
}

fn first_time(flag: &'static str, unit: Uuid, variant: &'static str) -> bool {
    static SEEN: OnceLock<Mutex<Seen>> = OnceLock::new();
    let mut seen = SEEN
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if seen.len() >= MAX_REMEMBERED {
        seen.clear();
    }
    seen.insert((flag, unit, variant))
}

#[cfg(test)]
mod tests {
    use super::first_time;
    use uuid::Uuid;

    #[test]
    fn a_unit_is_logged_once_per_variant() {
        let unit = Uuid::new_v4();
        assert!(first_time("exposure_test", unit, "on"));
        assert!(!first_time("exposure_test", unit, "on"));
        // A rules change that moves the unit is a new exposure.
        assert!(first_time("exposure_test", unit, "off"));
    }
}
