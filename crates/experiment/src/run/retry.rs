//! Resending failed flips. A failed flip keeps its row (its identity is the
//! idempotency key), so a retry resets that row to unsent rather than recording a
//! second one; how long to wait and how often to try is decided here.

use super::{AbortLevers, Failure, FlipFact, LeverState, RunAction, RunFacts, RunState};
use crate::config::{ExperimentConfig, OnClose};
use crate::moment::Moment;

/// Sends an `on` gets before the run stops retrying it: a lever that will not turn on
/// leaves the unit untreated, which the report counts. An `off` has no cap — a lever
/// left on is the failure that must not stand — and is retried until one lands.
pub const MAX_ON_ATTEMPTS: u32 = 3;
/// The wait after a failed attempt doubles from this, up to [`RETRY_MAX_HOURS`].
pub const RETRY_BASE_HOURS: i64 = 1;
pub const RETRY_MAX_HOURS: i64 = 8;

impl Failure {
    /// When the next attempt may go: 1h after the first failure, then 2h, 4h, 8h, 8h…
    pub fn retry_at(self) -> Moment {
        let doublings = self.attempts.saturating_sub(1).min(16);
        let hours = (RETRY_BASE_HOURS << doublings).min(RETRY_MAX_HOURS);
        Moment(self.at.at() + chrono::Duration::hours(hours))
    }
}

/// Every failed flip whose backoff has passed and that is still wanted: the unit's
/// latest flip (nothing sent or recorded since replaces it); an `off` always, an
/// `on` only when `ons` (the run is still treating), within [`MAX_ON_ATTEMPTS`], and
/// — under a switchback — while its period lasts.
pub(super) fn retries(c: &ExperimentConfig, f: &RunFacts, ons: bool) -> Vec<RunAction> {
    let superseded = |x: &FlipFact| {
        f.flips
            .iter()
            .any(|y| y.unit == x.unit && !y.cancelled && y.due_at > x.due_at)
    };
    f.flips
        .iter()
        .filter_map(|x| Some((x, x.failure?)))
        .filter(|(x, fail)| {
            x.state == LeverState::Off
                || (ons && fail.attempts < MAX_ON_ATTEMPTS && !period_ended(c, x.due_at, f.now))
        })
        .filter(|(x, fail)| !x.cancelled && !superseded(x) && f.now >= fail.retry_at())
        .map(|(x, _)| RunAction::RetryFlip {
            unit: x.unit.clone(),
            state: x.state,
            due_at: x.due_at,
        })
        .collect()
}

/// Off retries once the run has stopped treating — none when the levers are kept.
pub(super) fn close_retries(c: &ExperimentConfig, f: &RunFacts) -> Vec<RunAction> {
    match c.on_close() {
        OnClose::Keep => Vec::new(),
        OnClose::Off => retries(c, f, false),
    }
}

/// The switchback period that starts at `from` has ended by `now`.
fn period_ended(c: &ExperimentConfig, from: Moment, now: Moment) -> bool {
    let Some(sb) = &c.design.switchback else {
        return false;
    };
    c.design
        .grain
        .later(from, sb.period_days)
        .is_some_and(|until| until <= now)
}

/// A failed `off` the run in `state` would still resend: what keeps a settled run on the
/// short cadence. None once the levers are kept (`on_close: keep` at close, an abort
/// that kept them), since nothing is resent then.
pub fn owes_off_retry(c: &ExperimentConfig, state: RunState, f: &RunFacts) -> bool {
    let resends = match state {
        RunState::Registered | RunState::Live => true,
        RunState::Closing | RunState::Reported => matches!(c.on_close(), OnClose::Off),
        RunState::Aborted => f.abort_levers == AbortLevers::Off,
    };
    resends
        && f.flips.iter().any(|x| {
            x.state == LeverState::Off
                && x.failure.is_some()
                && !f
                    .flips
                    .iter()
                    .any(|y| y.unit == x.unit && !y.cancelled && y.due_at > x.due_at)
        })
}
