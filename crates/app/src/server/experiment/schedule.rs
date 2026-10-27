//! When a run next needs the tick: at most `RUN_CHECK_INTERVAL_SECS` away, and
//! exactly the zone's instant of the next switch moment (a wave's `switch_on`, or
//! a switchback period's start: a midnight, or a whole hour at hour grain) or of
//! the close when that is sooner. `day_start_utc` is PR 2's DST-tested function, so
//! a switch on a spring-forward day wakes at the first instant the day has.

use chrono::{DateTime, Duration, Utc};
use oxy_experiment::ExperimentConfig;
use oxy_experiment::Moment;
use oxy_experiment::run::{RunState, switch_moments};
use oxy_experiment::timeline::{last_post_window_end, moment_utc};

use super::jobs::{EstimateState, Retry};

pub const RUN_CHECK_INTERVAL_SECS: i64 = 300;
/// A settled terminal run is still looked at daily (cheap), never forgotten.
pub const SETTLED_RECHECK_SECS: i64 = 86_400;

pub fn next_check_at(
    c: &ExperimentConfig,
    state: RunState,
    local: Moment,
    now: DateTime<Utc>,
    work_left: bool,
) -> DateTime<Utc> {
    let soon = now + Duration::seconds(RUN_CHECK_INTERVAL_SECS);
    if state.is_terminal() {
        return if work_left {
            soon
        } else {
            now + Duration::seconds(SETTLED_RECHECK_SECS)
        };
    }
    let tz = c.design.timezone;
    switch_moments(c)
        .into_iter()
        .chain(last_post_window_end(c))
        .filter(|m| *m > local)
        .map(|m| moment_utc(m, tz))
        .filter(|at| *at > now)
        .min()
        .map_or(soon, |at| at.min(soon))
}

/// Whether a terminal run still owes its estimate, and so stays on the short
/// cadence: unqueued, queued or running, or failed with a retry waiting out its
/// backoff. Done, or failed with every attempt spent, settles to the daily
/// recheck — nothing the tick does can change it sooner.
pub fn estimate_owed(estimate: &EstimateState, retry: Option<Retry>) -> bool {
    match estimate {
        EstimateState::None | EstimateState::Running => true,
        EstimateState::Done => false,
        EstimateState::Failed { .. } => retry == Some(Retry::Wait),
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, NaiveDate, TimeZone, Utc};
    use oxy_experiment::run::RunState;
    use oxy_experiment::timeline::day_start_utc;

    use super::*;
    use crate::server::experiment::test_design::{design, switchback};

    fn d(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    #[test]
    fn a_switchback_wakes_at_the_first_instant_of_its_next_period() {
        let c = switchback();
        let start = day_start_utc(d("2026-03-09"), c.design.timezone);
        let now = start - Duration::minutes(2);
        assert_eq!(
            next_check_at(&c, RunState::Live, d("2026-03-08").into(), now, false),
            start
        );
    }

    #[test]
    fn a_run_waiting_for_its_switch_wakes_on_the_zones_first_instant_of_that_day() {
        let c = design();
        let start = day_start_utc(d("2026-03-02"), c.design.timezone);
        let now = start - Duration::minutes(2);
        assert_eq!(
            next_check_at(&c, RunState::Registered, d("2026-03-01").into(), now, false),
            start
        );
    }

    #[test]
    fn a_live_run_between_dates_rechecks_on_the_interval() {
        let c = design();
        let now = Utc.with_ymd_and_hms(2026, 3, 8, 3, 0, 0).unwrap();
        assert_eq!(
            next_check_at(&c, RunState::Live, d("2026-03-08").into(), now, false),
            now + Duration::seconds(RUN_CHECK_INTERVAL_SECS)
        );
    }

    #[test]
    fn a_settled_terminal_run_rests_a_day_and_one_with_work_left_does_not() {
        let c = design();
        let now = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
        assert_eq!(
            next_check_at(&c, RunState::Aborted, d("2026-05-01").into(), now, false),
            now + Duration::seconds(SETTLED_RECHECK_SECS)
        );
        assert_eq!(
            next_check_at(&c, RunState::Aborted, d("2026-05-01").into(), now, true),
            now + Duration::seconds(RUN_CHECK_INTERVAL_SECS)
        );
    }

    #[test]
    fn a_terminal_run_owes_its_estimate_until_it_is_done_or_exhausted() {
        let failed = || EstimateState::Failed {
            task_status: "failed".into(),
            error: None,
            failed_at: Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap(),
            attempts: 1,
        };
        assert!(estimate_owed(&EstimateState::None, None));
        assert!(estimate_owed(&EstimateState::Running, None));
        assert!(estimate_owed(&failed(), Some(Retry::Wait)));
        assert!(!estimate_owed(&EstimateState::Done, None));
        assert!(!estimate_owed(&failed(), Some(Retry::Exhausted)));
        assert!(!estimate_owed(&failed(), None));
    }
}
