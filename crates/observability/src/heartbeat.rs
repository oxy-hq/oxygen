//! Absence-of-traffic detection for custom apps.
//!
//! Pure: no I/O, no clock reads. Callers hand it two counts and it returns a
//! verdict, so the rule is unit-testable without a ClickHouse — same posture as
//! [`crate::burn_rate`].
//!
//! ## The hole this fills
//!
//! Every other signal the platform has divides failures by requests. That
//! arithmetic has a denominator, and the failure it cannot see is the one where
//! the denominator goes to zero: an app that breaks so thoroughly nobody can
//! reach it serves no failing requests, only no requests. The operator guide
//! names this case as uncovered by design — breakage with no deploy and no
//! traffic, "found by the first human in the morning".
//!
//! An app that served 200 requests in this window every week and zero today is
//! broken. Nothing that computes a ratio can ever say so.
//!
//! ## Why last week and not last hour
//!
//! These apps have weekly rhythms, not flat load. A store-ops app on a shared
//! tablet is busy at shift change and idle at 3am, and busy on Tuesday in a way
//! it never is on Sunday. Comparing this hour to the previous hour would call
//! every evening a silence. Comparing it to the *same phase one cycle back*
//! asks the only question with a stable answer: is this app doing what it does
//! at this time of week?
//!
//! That is the technique `.monitor.yml`'s Explain already uses — "compares the
//! same phase one cycle back (Monday vs prior Monday)" — so it is a known idea
//! in this codebase rather than a new primitive.
//!
//! ## Why strictly zero
//!
//! [`evaluate`] fires only on a *complete* absence, not on a large drop. A
//! fractional threshold ("80% below baseline") sounds more sensitive and is
//! mostly a holiday detector: a restaurant closed on a public holiday, one store
//! shut for refurbishment, a crew rostered off. Those are quiet, not broken, and
//! an alert that cries wolf on them is an alert nobody reads.
//!
//! Complete silence against an established baseline has no such innocent
//! reading, which is what makes it worth acting on.

/// How far back the comparison reaches: one week, so the window lands on the
/// same weekday and the same hour of that day.
pub const CYCLE_DAYS: u32 = 7;

/// The window compared, in minutes. Six hours — long enough that a shift
/// starting late or a delivery running early does not read as silence, short
/// enough to catch an overnight breakage before the morning.
///
/// Deliberately a member of [`crate::burn_rate::ALERT_WINDOWS_MINUTES`]: the
/// *current* side of the comparison then comes free from the availability
/// windows the caller already fetched, and only the baseline needs a query.
pub const WINDOW_MINUTES: u32 = 360;

/// Requests the same window must have carried a cycle ago before silence means
/// anything.
///
/// Below this there is no rhythm to have broken — an app that served three
/// requests last Tuesday and none this Tuesday has told us nothing. The number
/// matches [`crate::burn_rate::SloConfig::min_requests`] on purpose: the same
/// floor decides "enough traffic to have an opinion" in both places, and two
/// different floors would be two different definitions of quiet.
pub const MIN_BASELINE: u64 = 20;

/// What the heartbeat can say about one app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heartbeat {
    /// No established rhythm to compare against, so no opinion. **Not health** —
    /// an app nobody used last week is not an app confirmed working.
    NoBaseline,
    /// Traffic is flowing in the window.
    Beating,
    /// An established rhythm that has stopped: this window carried nothing,
    /// where a cycle ago it carried `expected`.
    Silent {
        /// What the same window carried one cycle back, for the reason line.
        expected: u64,
    },
}

/// Compare a window against the same window one [`CYCLE_DAYS`] cycle back.
///
/// `current_total` and `baseline_total` are request counts over the same
/// [`WINDOW_MINUTES`] span — the caller is responsible for them describing the
/// same phase, since this function cannot read a clock to check.
pub fn evaluate(current_total: u64, baseline_total: u64) -> Heartbeat {
    if baseline_total < MIN_BASELINE {
        return Heartbeat::NoBaseline;
    }
    if current_total > 0 {
        return Heartbeat::Beating;
    }
    Heartbeat::Silent {
        expected: baseline_total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The case the whole module exists for, and the one no ratio rule can see.
    #[test]
    fn an_established_app_serving_nothing_is_silent() {
        assert_eq!(evaluate(0, 200), Heartbeat::Silent { expected: 200 });
    }

    /// An app nobody used last week says nothing about this week. This must not
    /// read as silence, or every unused app in the fleet alerts forever.
    #[test]
    fn no_baseline_is_not_silence() {
        assert_eq!(evaluate(0, 0), Heartbeat::NoBaseline);
        assert_eq!(evaluate(0, MIN_BASELINE - 1), Heartbeat::NoBaseline);
    }

    /// The floor is inclusive, and it is the same number the burn evaluator
    /// uses. If these ever drift, "quiet" means two different things depending
    /// on which module you ask.
    #[test]
    fn the_baseline_floor_matches_the_burn_evaluators_traffic_floor() {
        assert_eq!(
            MIN_BASELINE,
            crate::burn_rate::SloConfig::default().min_requests
        );
        assert_eq!(
            evaluate(0, MIN_BASELINE),
            Heartbeat::Silent { expected: 20 }
        );
    }

    /// Strictly zero, not "a lot fewer". A 95% drop against a big baseline is a
    /// holiday as often as an outage; see the module docs.
    #[test]
    fn a_large_drop_is_not_silence() {
        assert_eq!(evaluate(1, 1000), Heartbeat::Beating);
    }

    /// The window is one the availability query already fetches, so the current
    /// side of the comparison costs nothing extra.
    #[test]
    fn the_window_is_one_the_availability_query_already_returns() {
        assert!(crate::burn_rate::ALERT_WINDOWS_MINUTES.contains(&WINDOW_MINUTES));
    }
}
