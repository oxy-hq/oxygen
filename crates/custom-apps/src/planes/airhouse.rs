//! The Airhouse plane's checks: where its overlay modes may run, and how old
//! a `snapshot` pin may be
//! (`internal-docs/2026-11-02-custom-app-branches-design.md` §5.4).
//!
//! `branch` and `snapshot` copy production rows into the environment's
//! sibling, so they fall under the 30-day commitment on copies (§4): only a
//! sandbox, which the maintenance loop tears down 30 days past its fork point,
//! may run them. Staging is never torn down, so it keeps `write-only`.

use chrono::{DateTime, Duration, Utc};
use oxy_app_core::custom_app_environment::AppEnvironment;

use super::AirhouseMode;

/// Hours a `snapshot` pin stays readable. Airhouse expires DuckLake
/// snapshots older than its retention — one day on our deployments (the
/// compaction drain's `expireRetentionDays: 1`), not the chart's thirty — so
/// a pin older than this reads a snapshot that may be gone.
pub const SNAPSHOT_WINDOW_VAR: &str = "OXY_AIRHOUSE_SNAPSHOT_WINDOW_HOURS";
pub const DEFAULT_SNAPSHOT_WINDOW_HOURS: i64 = 24;

/// How long a `snapshot` pin stays readable: [`SNAPSHOT_WINDOW_VAR`] hours,
/// or [`DEFAULT_SNAPSHOT_WINDOW_HOURS`]. A value that is not a positive
/// whole number of hours is ignored, with a warning.
pub fn snapshot_window() -> Duration {
    Duration::hours(parse_window_hours(
        std::env::var(SNAPSHOT_WINDOW_VAR).ok().as_deref(),
    ))
}

pub(super) fn parse_window_hours(value: Option<&str>) -> i64 {
    match value.map(str::trim) {
        None | Some("") => DEFAULT_SNAPSHOT_WINDOW_HOURS,
        Some(v) => match v.parse::<i64>() {
            Ok(hours) if hours > 0 => hours,
            _ => {
                tracing::warn!(
                    value = v,
                    "{SNAPSHOT_WINDOW_VAR} is not a positive number of hours; using the default"
                );
                DEFAULT_SNAPSHOT_WINDOW_HOURS
            }
        },
    }
}

/// Why a `snapshot` read pinned at `pin` cannot run at `now`, or `None` when
/// it can. A pin in the future is refused too: DuckLake refuses to read one.
pub fn pin_refusal(pin: DateTime<Utc>, now: DateTime<Utc>, window: Duration) -> Option<String> {
    if pin > now {
        return Some(format!("the Airhouse snapshot pin {pin} is in the future"));
    }
    (now - pin > window).then(|| {
        format!(
            "the Airhouse snapshot pin {pin} is older than {} hours, and Airhouse may already \
             have expired that snapshot",
            window.num_hours()
        )
    })
}

/// The airhouse arm of `PlaneSpec::validate`: the overlay modes on a sandbox
/// only, and a `snapshot` sandbox's fork point within the window, since its
/// first read pins there.
pub(super) fn check(
    mode: AirhouseMode,
    environment: &AppEnvironment,
    fork_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<(), String> {
    if mode == AirhouseMode::WriteOnly {
        return Ok(());
    }
    if !matches!(environment, AppEnvironment::Dev { .. }) {
        return Err(format!(
            "airhouse={} is for sandboxes only: it copies production rows, which may be kept \
             at most 30 days, and {environment} is never torn down. Keep airhouse=write-only \
             here, or run the overlay in a dev-<handle> sandbox",
            mode.as_str()
        ));
    }
    match (mode, fork_at) {
        (AirhouseMode::Snapshot, Some(at)) => match pin_refusal(at, now, snapshot_window()) {
            Some(why) => Err(format!(
                "airhouse=snapshot reads production at the fork point {at}: {why}. Cut the \
                 sandbox with --at within the last {} hours, or without --at to pin now",
                snapshot_window().num_hours()
            )),
            None => Ok(()),
        },
        _ => Ok(()),
    }
}

#[cfg(test)]
#[path = "airhouse_tests.rs"]
mod tests;
