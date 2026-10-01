//! Airway samples (phase 2b S11, P4): staff run a bounded window of a previewed
//! branch's pipeline into the preview's own Airhouse schemas, to see what the
//! branch loads before it is merged.
//!
//! * **Submit** ([`validate`]) — `POST /previews/runs` with `kind:
//!   "airway_sample"`. The rules are `agentic_airway::preview::SamplePolicy`'s:
//!   `postgres_cdc`, `pgoutput` and `sp_api` are never sampled (`422
//!   sample_refused`); a windowed source takes a window of at most 31 days
//!   (default the last 7); any other names its `resources` when it has more
//!   than one, and runs under a wall-clock cap instead; a rotate-on-use source
//!   (QuickBooks) needs a sandbox company registered for the pipeline
//!   (`PUT /previews/sources`, [`super::sources`]) or answers `409
//!   sandbox_required`. The destination must be the workspace's managed
//!   Airhouse.
//! * **Seed** ([`seed`]) — when the workspace's preview queue frees up
//!   (`runs::advance`), an `agentic_runs` row (`source_type =
//!   preview_airway_sample`, which renders its Airway events) and a
//!   `TaskSpec::Custom` on the worker fleet, in the queue's transaction.
//! * **Run** ([`PreviewAirwaySampleExecutor`]) — the preview platform for the
//!   run (its destination maps the dataset into `preview_<key>__<dataset>`
//!   and `_raw`, on a Writer confined to those; its secrets withhold
//!   production's QuickBooks vars and write only the sandbox's rotating one)
//!   drives `execute_airway_preview_sample`, which renames the pipeline
//!   `preview:<key>:<name>`: its own lease, cursor, stored schema, load audit
//!   and run extension. Production's rows are never read or written.
//! * **After** ([`outcome`], [`record`]) — the sample is cut off at its
//!   deadline (the cap, or just under the run ceiling) and then reads
//!   `partial`; its tables are recorded in the preview's shadow map
//!   (`state = 'sample'`, so the TTL drop takes them and later preview runs
//!   read them), and its stored schema is compared with production's.
//!
//! An Airhouse that cannot confine a Writer (older than 0.1.49) refuses the
//! sample before anything is written; it never lands live.

mod executor;
mod outcome;
mod record;
mod seed;
#[cfg(test)]
mod tests;
mod validate;
mod view;

use std::time::Duration;

pub use agentic_pipeline::PREVIEW_AIRWAY_SAMPLE as PREVIEW_AIRWAY_SAMPLE_KIND;
pub use executor::PreviewAirwaySampleExecutor;
pub use record::{Recorded, SampleReport, record_sample};
pub use seed::{SampleSeed, seed};
pub use validate::{Asked, SampleOptions, check, validate};
pub use view::view;

/// The `workspace_preview_runs.kind` of a sample.
pub const RUN_KIND: &str = crate::agentic_wiring::preview_ctx::SAMPLE_KIND;

/// Wall-clock cap, in seconds, on a sample of a source with no date window.
pub const MAX_SECS_ENV: &str = "OXY_PREVIEW_SAMPLE_MAX_SECS";
const DEFAULT_MAX_SECS: u64 = 900;

/// How far under the run ceiling (`OXY_PREVIEW_RUN_MAX_MINUTES`) every sample
/// is cut off, so its engine has stopped and released its lease before the
/// ceiling retires the run and lets the workspace's next run start.
const CEILING_MARGIN: Duration = Duration::from_secs(5 * 60);

/// The cap: the env value when it is a positive integer.
pub fn max_secs() -> u64 {
    std::env::var(MAX_SECS_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&s| s > 0)
        .unwrap_or(DEFAULT_MAX_SECS)
}

/// When a sample is cut off, and what cut it off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Deadline {
    pub after: Duration,
    pub reason: &'static str,
}

/// What is left of `deadline` for a sample whose preview run started at
/// `started_at` (the queue step's `started_at`, which a re-claimed task does
/// not reset): the clock runs from the run's start, not from this claim.
pub fn remaining(
    deadline: Deadline,
    started_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    now: chrono::DateTime<chrono::Utc>,
) -> Deadline {
    let elapsed = started_at
        .and_then(|s| (now - s.with_timezone(&chrono::Utc)).to_std().ok())
        .unwrap_or_default();
    Deadline {
        after: deadline.after.saturating_sub(elapsed),
        reason: deadline.reason,
    }
}

/// A capped sample stops at `cap_secs`; every sample stops [`CEILING_MARGIN`]
/// before the run ceiling at the latest (a minute at least).
pub fn deadline(capped: bool, cap_secs: u64, ceiling_minutes: i64) -> Deadline {
    let ceiling = Duration::from_secs(ceiling_minutes.max(1) as u64 * 60)
        .saturating_sub(CEILING_MARGIN)
        .max(Duration::from_secs(60));
    let cap = Duration::from_secs(cap_secs);
    if capped && cap < ceiling {
        Deadline {
            after: cap,
            reason: "the sample's wall-clock cap (OXY_PREVIEW_SAMPLE_MAX_SECS)",
        }
    } else {
        Deadline {
            after: ceiling,
            reason: "the preview run ceiling (OXY_PREVIEW_RUN_MAX_MINUTES)",
        }
    }
}
