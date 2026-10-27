//! The experiment's grain: the calendar period one panel cell covers.
//!
//! Every window count in a design (`pre_days`, `post_days`, `washout_days`, …)
//! is a number of periods of this grain, and every date a design names (a
//! wave's `switch_on`, a switchback period's `from`) is the first day of one.
//! The keys keep their `_days` spelling so a day-grain file reads, and hashes,
//! as it always has.
//!
//! The estimator never sees a date: the host numbers each period with
//! [`Grain::ordinal`], consecutive periods getting consecutive integers, so a
//! window of `n` periods is `n` ordinals whatever the grain. A week is the ISO
//! week (Monday first), a month the calendar month, both in `design.timezone`.

use chrono::{Datelike, Days, Months, NaiveDate, NaiveDateTime, Timelike};
use serde::{Deserialize, Serialize};

use crate::moment::Moment;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grain {
    /// Hourly readings: only on a datetime time dimension. A date stands for its
    /// first hour (local midnight).
    Hour,
    #[default]
    Day,
    Week,
    Month,
}

impl Grain {
    pub const ALL: [Grain; 3] = [Grain::Day, Grain::Week, Grain::Month];

    /// The semantic query's granularity name.
    pub fn as_str(self) -> &'static str {
        match self {
            Grain::Hour => "hour",
            Grain::Day => "day",
            Grain::Week => "week",
            Grain::Month => "month",
        }
    }

    /// `skip_serializing_if`: a day-grain design serializes without the key.
    pub fn is_day(&self) -> bool {
        *self == Grain::Day
    }

    /// The fewest calendar days one period spans.
    pub fn min_days(self) -> u32 {
        match self {
            Grain::Hour | Grain::Day => 1,
            Grain::Week => 7,
            Grain::Month => 28,
        }
    }

    /// The most calendar days one period spans.
    pub fn max_days(self) -> u32 {
        match self {
            Grain::Hour | Grain::Day => 1,
            Grain::Week => 7,
            Grain::Month => 31,
        }
    }

    /// The fewest whole periods that cover `days` calendar days.
    pub fn periods_covering(self, days: u32) -> u32 {
        match self {
            Grain::Hour => days.saturating_mul(24),
            _ => days.div_ceil(self.min_days()),
        }
    }

    /// The most calendar days `n` periods from a period start can span.
    pub fn days_spanned(self, n: u32) -> u32 {
        match self {
            Grain::Hour => n.div_ceil(24),
            _ => n.saturating_mul(self.max_days()),
        }
    }

    /// The first date at or after `d` plus `n` periods. At hour grain a length
    /// that ends mid-day rounds up to the next midnight: the lever and the
    /// runner move on dates, and a reading stops at its own last hour anyway.
    pub fn add_covering(self, d: NaiveDate, n: u32) -> Option<NaiveDate> {
        match self {
            Grain::Hour => d.checked_add_days(Days::new(u64::from(n.div_ceil(24)))),
            _ => self.add(d, n),
        }
    }

    /// The last date at or before `d` minus `n` periods (the hour twin of
    /// [`Grain::add_covering`], rounding down).
    pub fn sub_covering(self, d: NaiveDate, n: u32) -> Option<NaiveDate> {
        match self {
            Grain::Hour => d.checked_sub_days(Days::new(u64::from(n.div_ceil(24)))),
            _ => self.sub(d, n),
        }
    }

    /// The number of the period `d` falls in. Consecutive periods are
    /// consecutive integers; for a day it is `num_days_from_ce`, the
    /// estimator's own day ordinal, so a day-grain panel is unchanged.
    pub fn ordinal(self, d: NaiveDate) -> i64 {
        let days = i64::from(d.num_days_from_ce());
        match self {
            Grain::Hour => days * 24,
            Grain::Day => days,
            // 0001-01-01 (day 1) is a Monday.
            Grain::Week => (days - 1).div_euclid(7),
            Grain::Month => i64::from(d.year()) * 12 + i64::from(d.month0()),
        }
    }

    /// The first day of period `ord`. `None` past chrono's calendar.
    pub fn start_of(self, ord: i64) -> Option<NaiveDate> {
        match self {
            Grain::Hour => {
                NaiveDate::from_num_days_from_ce_opt(i32::try_from(ord.div_euclid(24)).ok()?)
            }
            Grain::Day => NaiveDate::from_num_days_from_ce_opt(i32::try_from(ord).ok()?),
            Grain::Week => {
                NaiveDate::from_num_days_from_ce_opt(i32::try_from(ord.checked_mul(7)? + 1).ok()?)
            }
            Grain::Month => {
                let year = i32::try_from(ord.div_euclid(12)).ok()?;
                let month = u32::try_from(ord.rem_euclid(12)).ok()? + 1;
                NaiveDate::from_ymd_opt(year, month, 1)
            }
        }
    }

    /// The number of the period the local moment `at` falls in: its hour at hour
    /// grain, its date's period otherwise.
    pub fn ordinal_at(self, at: NaiveDateTime) -> i64 {
        match self {
            Grain::Hour => self.ordinal(at.date()) + i64::from(at.hour()),
            _ => self.ordinal(at.date()),
        }
    }

    /// The first moment of period `ord`.
    pub fn start_at(self, ord: i64) -> Option<NaiveDateTime> {
        let date = self.start_of(ord)?;
        let hour = match self {
            Grain::Hour => u32::try_from(ord.rem_euclid(24)).ok()?,
            _ => 0,
        };
        date.and_hms_opt(hour, 0, 0)
    }

    /// The number of the period a switch moment opens: its hour at hour grain.
    pub fn ordinal_of(self, m: Moment) -> i64 {
        self.ordinal_at(m.at())
    }

    /// Whether `m` starts a period: a whole hour at hour grain, else the first
    /// midnight of a day, week or month.
    pub fn starts(self, m: Moment) -> bool {
        match self {
            Grain::Hour => m.is_whole_hour(),
            _ => m.is_midnight() && self.is_start(m.date()),
        }
    }

    /// `m` moved `n` periods later: `n` hours at hour grain, else `n` periods of
    /// its date at the same time of day.
    pub fn later(self, m: Moment, n: u32) -> Option<Moment> {
        match self {
            Grain::Hour => m.plus_hours(i64::from(n)),
            _ => Some(Moment(self.add(m.date(), n)?.and_time(m.at().time()))),
        }
    }

    /// `m` moved `n` periods earlier.
    pub fn earlier(self, m: Moment, n: u32) -> Option<Moment> {
        match self {
            Grain::Hour => m.plus_hours(-i64::from(n)),
            _ => Some(Moment(self.sub(m.date(), n)?.and_time(m.at().time()))),
        }
    }

    /// The first day of the period `d` falls in.
    pub fn floor(self, d: NaiveDate) -> NaiveDate {
        self.start_of(self.ordinal(d))
            .expect("the start of a date's own period is a date")
    }

    /// Whether `d` is the first day of its period.
    pub fn is_start(self, d: NaiveDate) -> bool {
        self.floor(d) == d
    }

    /// `d` moved `n` periods later. For a month, the same day of the month
    /// (clamped, as chrono does); on a period start that is a period start.
    pub fn add(self, d: NaiveDate, n: u32) -> Option<NaiveDate> {
        match self {
            // A date moves only by whole days of hours.
            Grain::Hour => n
                .is_multiple_of(24)
                .then(|| d.checked_add_days(Days::new(u64::from(n / 24))))?,
            Grain::Day => d.checked_add_days(Days::new(u64::from(n))),
            Grain::Week => d.checked_add_days(Days::new(u64::from(n) * 7)),
            Grain::Month => d.checked_add_months(Months::new(n)),
        }
    }

    /// `d` moved `n` periods earlier.
    pub fn sub(self, d: NaiveDate, n: u32) -> Option<NaiveDate> {
        match self {
            Grain::Hour => n
                .is_multiple_of(24)
                .then(|| d.checked_sub_days(Days::new(u64::from(n / 24))))?,
            Grain::Day => d.checked_sub_days(Days::new(u64::from(n))),
            Grain::Week => d.checked_sub_days(Days::new(u64::from(n) * 7)),
            Grain::Month => d.checked_sub_months(Months::new(n)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn a_day_ordinal_is_the_estimators_day_ordinal() {
        let day = d("2026-03-02");
        assert_eq!(Grain::Day.ordinal(day), i64::from(day.num_days_from_ce()));
        assert_eq!(Grain::Day.start_of(Grain::Day.ordinal(day)), Some(day));
    }

    #[test]
    fn every_day_of_an_iso_week_shares_its_monday_ordinal_and_the_next_week_is_one_more() {
        let monday = d("2026-10-19");
        let ord = Grain::Week.ordinal(monday);
        for k in 0..7 {
            assert_eq!(Grain::Week.ordinal(monday + Days::new(k)), ord);
        }
        assert_eq!(Grain::Week.ordinal(d("2026-10-26")), ord + 1);
        assert_eq!(Grain::Week.ordinal(d("2026-10-18")), ord - 1);
        assert_eq!(Grain::Week.start_of(ord), Some(monday));
        assert_eq!(Grain::Week.floor(d("2026-10-25")), monday);
    }

    #[test]
    fn months_are_consecutive_across_a_year_end() {
        let dec = Grain::Month.ordinal(d("2025-12-31"));
        assert_eq!(Grain::Month.ordinal(d("2026-01-01")), dec + 1);
        assert_eq!(Grain::Month.start_of(dec + 1), Some(d("2026-01-01")));
        assert_eq!(Grain::Month.floor(d("2026-02-17")), d("2026-02-01"));
    }

    #[test]
    fn a_period_start_is_recognised_per_grain() {
        assert!(Grain::Day.is_start(d("2026-10-21")));
        assert!(Grain::Week.is_start(d("2026-10-19")));
        assert!(!Grain::Week.is_start(d("2026-10-21")));
        assert!(Grain::Month.is_start(d("2026-10-01")));
        assert!(!Grain::Month.is_start(d("2026-10-02")));
    }

    #[test]
    fn adding_periods_from_a_start_lands_on_the_start_n_ordinals_later() {
        for g in Grain::ALL {
            let start = g.floor(d("2026-01-31"));
            let later = g.add(start, 5).unwrap();
            assert!(g.is_start(later), "{g:?}");
            assert_eq!(g.ordinal(later), g.ordinal(start) + 5, "{g:?}");
            assert_eq!(g.sub(later, 5), Some(start), "{g:?}");
        }
    }

    #[test]
    fn hours_are_consecutive_across_midnight_and_a_day_starts_at_its_first_hour() {
        let at = |s: &str| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap();
        let last = Grain::Hour.ordinal_at(at("2026-10-19 23:00"));
        assert_eq!(Grain::Hour.ordinal_at(at("2026-10-20 00:00")), last + 1);
        assert_eq!(Grain::Hour.ordinal_at(at("2026-10-20 00:59")), last + 1);
        assert_eq!(Grain::Hour.ordinal(d("2026-10-20")), last + 1);
        assert_eq!(Grain::Hour.start_at(last + 1), Some(at("2026-10-20 00:00")));
        // A coarser grain reads a moment by its date.
        assert_eq!(
            Grain::Week.ordinal_at(at("2026-10-25 23:00")),
            Grain::Week.ordinal(d("2026-10-19"))
        );
    }

    #[test]
    fn hour_lengths_round_out_to_the_whole_days_they_cover() {
        assert_eq!(
            Grain::Hour.add_covering(d("2026-03-16"), 36),
            Some(d("2026-03-18"))
        );
        assert_eq!(
            Grain::Hour.add_covering(d("2026-03-16"), 48),
            Some(d("2026-03-18"))
        );
        assert_eq!(
            Grain::Hour.sub_covering(d("2026-03-16"), 3),
            Some(d("2026-03-15"))
        );
        assert_eq!(
            Grain::Week.add_covering(d("2026-03-16"), 2),
            Some(d("2026-03-30"))
        );
        assert_eq!(Grain::Hour.periods_covering(28), 28 * 24);
        assert_eq!(Grain::Hour.days_spanned(30), 2);
        assert_eq!(Grain::Week.days_spanned(2), 14);
    }

    #[test]
    fn a_lookback_in_days_becomes_the_whole_periods_that_cover_it() {
        assert_eq!(Grain::Day.periods_covering(28), 28);
        assert_eq!(Grain::Week.periods_covering(28), 4);
        assert_eq!(Grain::Week.periods_covering(29), 5);
        assert_eq!(Grain::Month.periods_covering(28), 1);
    }

    #[test]
    fn a_day_grain_design_serializes_without_the_key_and_reads_back_as_day() {
        assert_eq!(serde_json::to_value(Grain::Week).unwrap(), "week");
        assert_eq!(
            serde_json::from_value::<Grain>("month".into()).unwrap(),
            Grain::Month
        );
        assert!(Grain::default().is_day());
    }
}
