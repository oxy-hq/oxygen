//! The week a report covers.

use chrono::{DateTime, Datelike, Duration, NaiveTime, Utc};
use serde::{Deserialize, Serialize};

/// One reported week: `[start, end)`, from a Monday 00:00 UTC to the next.
///
/// UTC rather than anyone's local week: the report covers every org at once, and
/// a week that started at a different instant per reader could not be stored once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Period {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl Period {
    /// The last week that had fully ended at `now`. At Monday 00:00:00 exactly,
    /// that is the week that ended this instant.
    pub fn last_completed(now: DateTime<Utc>) -> Self {
        let days_into_week = i64::from(now.weekday().num_days_from_monday());
        let monday = now.date_naive() - Duration::days(days_into_week);
        let end = monday.and_time(NaiveTime::MIN).and_utc();
        Self {
            start: end - Duration::weeks(1),
            end,
        }
    }

    /// The week before this one — what every count is compared with.
    pub fn previous(&self) -> Self {
        Self {
            start: self.start - Duration::weeks(1),
            end: self.start,
        }
    }

    /// `Sep 28 – Oct 4, 2026`. The last day named is the last day covered, not
    /// the exclusive end.
    pub fn label(&self) -> String {
        format!("{}, {}", self.short_label(), self.last_day().format("%Y"))
    }

    /// `Sep 28 – Oct 4`, for a subject line.
    pub fn short_label(&self) -> String {
        format!(
            "{} – {}",
            self.start.format("%b %-d"),
            self.last_day().format("%b %-d")
        )
    }

    fn last_day(&self) -> DateTime<Utc> {
        self.end - Duration::days(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, m: u32, d: u32, h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, 0, 0).unwrap()
    }

    #[test]
    fn the_last_completed_week_runs_monday_to_monday() {
        // 2026-10-06 is a Tuesday.
        let period = Period::last_completed(at(2026, 10, 6, 15));
        assert_eq!(period.start, at(2026, 9, 28, 0));
        assert_eq!(period.end, at(2026, 10, 5, 0));
    }

    #[test]
    fn every_day_of_a_week_names_the_same_completed_week() {
        let expected = Period::last_completed(at(2026, 10, 5, 0));
        for day in 5..=11 {
            assert_eq!(Period::last_completed(at(2026, 10, day, 23)), expected);
        }
        // The next Monday moves on by exactly one week.
        let next = Period::last_completed(at(2026, 10, 12, 0));
        assert_eq!(next.start, expected.end);
    }

    #[test]
    fn a_sunday_still_belongs_to_the_week_in_progress() {
        // 2026-10-11 is a Sunday: its own week has not ended.
        let period = Period::last_completed(at(2026, 10, 11, 23));
        assert_eq!(period.end, at(2026, 10, 5, 0));
    }

    #[test]
    fn the_previous_week_ends_where_this_one_starts() {
        let period = Period::last_completed(at(2026, 10, 6, 0));
        let previous = period.previous();
        assert_eq!(previous.end, period.start);
        assert_eq!(previous.start, at(2026, 9, 21, 0));
    }

    #[test]
    fn the_label_names_the_last_day_covered() {
        let period = Period::last_completed(at(2026, 10, 6, 0));
        assert_eq!(period.label(), "Sep 28 – Oct 4, 2026");
        assert_eq!(period.short_label(), "Sep 28 – Oct 4");
    }
}
