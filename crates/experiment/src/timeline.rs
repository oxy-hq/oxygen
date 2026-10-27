//! Everything calendar, in `design.timezone`.
//!
//! Every `switch_on`, window boundary and "today" in an experiment is a calendar
//! date in one IANA zone; this module is the only place a date becomes an
//! instant. Two conventions, fixed by index C2 and consumed by later PRs:
//! [`registration_deadline`] is the *start* of a local day and a registering
//! commit must be strictly before it; [`last_post_window_end`] is *exclusive*.

use crate::config::ExperimentConfig;
use crate::grain::Grain;
use crate::moment::Moment;
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use tracing::warn;

/// The longest run of local minutes searched for a first valid instant. A gap
/// is at most an hour in practice; Pacific/Apia skipped a whole date (2011-12-30),
/// which is 1440 minutes — twice that leaves room and stays bounded.
const MAX_SKIPPED_MINUTES: i64 = 2 * 24 * 60;

/// The earliest instant whose local calendar date in `tz` is on or after `d`.
///
/// Local midnight when it exists once; the *first* occurrence when it occurs
/// twice (clocks set back over midnight); the first valid local instant when it
/// does not exist (clocks set forward over midnight, e.g. São Paulo 2018-11-04,
/// Beirut 2022-03-27); and the instant the next date begins when the whole date
/// was skipped (Apia 2011-12-30).
pub fn day_start_utc(d: NaiveDate, tz: Tz) -> DateTime<Utc> {
    let midnight = d.and_time(NaiveTime::MIN);
    // `earliest()` is `Some` for a unique local time and for the first of two.
    match tz.from_local_datetime(&midnight).earliest() {
        Some(t) => t.with_timezone(&Utc),
        None => first_local_instant_after(midnight, tz),
    }
}

fn first_local_instant_after(midnight: NaiveDateTime, tz: Tz) -> DateTime<Utc> {
    for minutes in 1..=MAX_SKIPPED_MINUTES {
        let Some(t) = midnight.checked_add_signed(chrono::Duration::minutes(minutes)) else {
            break;
        };
        if let Some(found) = tz.from_local_datetime(&t).earliest() {
            return found.with_timezone(&Utc);
        }
    }
    // Unreachable with real tzdata; degrade to UTC midnight rather than panic.
    warn!(%midnight, %tz, "no valid local instant found after a nonexistent midnight");
    Utc.from_utc_datetime(&midnight)
}

/// The calendar date `now` falls on in `tz`.
pub fn today_in(tz: Tz, now: DateTime<Utc>) -> NaiveDate {
    now.with_timezone(&tz).date_naive()
}

/// The local moment `now` falls on in `tz`, floored to the hour: switches are
/// whole hours (or midnights), so a flip is due once this reaches it.
pub fn local_hour_in(tz: Tz, now: DateTime<Utc>) -> Moment {
    let local = now.with_timezone(&tz).naive_local();
    let hour = local
        .date()
        .and_hms_opt(chrono::Timelike::hour(&local), 0, 0)
        .unwrap_or(local);
    Moment(hour)
}

/// The instant a switch moment starts in `tz` (see [`day_start_utc`] for the
/// edges: a skipped local time starts at the first instant after it).
pub fn moment_utc(m: Moment, tz: Tz) -> DateTime<Utc> {
    if m.is_midnight() {
        day_start_utc(m.date(), tz)
    } else {
        local_instant_utc(m.at(), tz)
    }
}

/// The earliest moment the lever moves: the first `switch_on`, or the first period
/// `from` of a switchback. `None` for a draft.
pub fn first_switch(c: &ExperimentConfig) -> Option<Moment> {
    let waves = c.design.waves.iter().map(|w| w.switch_on);
    let periods = c
        .design
        .switchback
        .iter()
        .flat_map(|s| s.periods.iter().map(|p| p.from));
    waves.chain(periods).min()
}

/// When registration closes: the start of the local day
/// `first switch − anticipation_days` (periods of `design.grain`) (the first wave's `switch_on`, or the first
/// switchback period's `from`). A registering commit must be strictly before
/// this instant. `None` for a draft (no waves, no periods) **and** when the date
/// arithmetic overflows (an absurd `anticipation_days`), so a caller must gate on
/// [`crate::is_registrable`], never on `Some`/`None`.
pub fn registration_deadline(c: &ExperimentConfig) -> Option<DateTime<Utc>> {
    deadline_before(c, first_switch(c)?)
}

/// The instant `anticipation_days` periods before the switch moment `first`. At
/// hour grain that is `anticipation_days` hours before it, mid-day as often as not.
pub fn deadline_before(c: &ExperimentConfig, first: Moment) -> Option<DateTime<Utc>> {
    let (grain, tz) = (c.design.grain, c.design.timezone);
    if grain == Grain::Hour {
        return Some(moment_utc(
            grain.earlier(first, c.design.anticipation_days)?,
            tz,
        ));
    }
    Some(day_start_utc(
        grain.sub(first.date(), c.design.anticipation_days)?,
        tz,
    ))
}

/// The earliest instant at or after the local moment `at` in `tz` (a skipped
/// hour starts at the first instant after it), as [`day_start_utc`] does for midnight.
pub fn local_instant_utc(at: NaiveDateTime, tz: Tz) -> DateTime<Utc> {
    match tz.from_local_datetime(&at).earliest() {
        Some(t) => t.with_timezone(&Utc),
        None => first_local_instant_after(at, tz),
    }
}

/// The **exclusive** end of the last post window. Waves:
/// `max(switch_on) + washout_days + post_days` periods of `design.grain`. Switchback: the day
/// after the last period, `max(from) + period_days`. The last day inside the
/// window is the day before. `None` for a draft, for a waves design with no
/// `post_days`, **and** when the date arithmetic overflows (absurd `post_days` /
/// `washout_days` / `period_days`), so a caller must gate on
/// [`crate::is_registrable`], never on `Some`/`None`. A config with both shapes is unreachable once compiled
/// (`WavesAndSwitchback` refuses it); behaviour there is unspecified.
///
/// A moment: at hour grain the window can end at any hour. A reader counting
/// whole days takes [`last_post_window_end_date`].
pub fn last_post_window_end(c: &ExperimentConfig) -> Option<Moment> {
    let grain = c.design.grain;
    if !c.design.waves.is_empty() {
        let last = c.design.waves.iter().map(|w| w.switch_on).max()?;
        let span = c.design.washout_days.checked_add(c.design.post_days?)?;
        return grain.later(last, span);
    }
    let sb = c.design.switchback.as_ref()?;
    let last = sb.periods.iter().map(|p| p.from).max()?;
    grain.later(last, sb.period_days)
}

/// [`last_post_window_end`] rounded out to the next midnight: the exclusive end
/// a reader of whole days (panel bounds, daily read-back) stops at.
pub fn last_post_window_end_date(c: &ExperimentConfig) -> Option<NaiveDate> {
    last_post_window_end(c).map(Moment::ceil_date)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WaveConfig;
    use crate::test_fixtures::{date, draft, full, switchback, switchback_draft};
    use chrono::{Datelike, Duration, TimeZone};
    use chrono_tz::America::{Havana, New_York, Sao_Paulo};
    use chrono_tz::Asia::Beirut;
    use chrono_tz::Australia::Sydney;
    use chrono_tz::Pacific::Apia;

    fn utc(y: i32, m: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, mi, 0).unwrap()
    }

    fn wave(y: i32, m: u32, d: u32) -> WaveConfig {
        WaveConfig {
            switch_on: date(y, m, d).into(),
            units: vec!["u".into()],
        }
    }

    #[test]
    fn dst_transition_day_starts_at_the_first_local_instant() {
        // Review Focus 2. (date, zone, expected UTC instant, why it is interesting)
        let cases: [(NaiveDate, Tz, DateTime<Utc>, &str); 9] = [
            (
                date(2026, 10, 4),
                Sydney,
                utc(2026, 10, 3, 14, 0),
                "AU spring-forward day (gap 02:00→03:00): midnight exists, +10",
            ),
            (
                date(2026, 10, 5),
                Sydney,
                utc(2026, 10, 4, 13, 0),
                "the day after, +11",
            ),
            (
                date(2027, 4, 4),
                Sydney,
                utc(2027, 4, 3, 13, 0),
                "AU fall-back day (03:00→02:00): midnight exists once, +11",
            ),
            (
                date(2026, 3, 8),
                New_York,
                utc(2026, 3, 8, 5, 0),
                "US spring-forward day: midnight exists, -5",
            ),
            (
                date(2026, 3, 9),
                New_York,
                utc(2026, 3, 9, 4, 0),
                "the day after, -4",
            ),
            (
                date(2018, 11, 4),
                Sao_Paulo,
                utc(2018, 11, 4, 3, 0),
                "midnight does not exist (00:00→01:00): first instant is 01:00 local",
            ),
            (
                date(2022, 3, 27),
                Beirut,
                utc(2022, 3, 26, 22, 0),
                "midnight does not exist (00:00→01:00): first instant is 01:00 local",
            ),
            (
                date(2022, 11, 6),
                Havana,
                utc(2022, 11, 6, 4, 0),
                "midnight occurs twice (01:00→00:00): the FIRST one",
            ),
            (
                date(2011, 12, 30),
                Apia,
                utc(2011, 12, 30, 10, 0),
                "the whole date was skipped: the instant the next date begins",
            ),
        ];
        for (d, tz, expected, why) in cases {
            let got = day_start_utc(d, tz);
            assert_eq!(got, expected, "{tz} {d}: {why}");
            // The definition, independent of the hand-computed instants above:
            // `got` is the earliest instant whose local date is on or after `d`.
            assert!(
                got.with_timezone(&tz).date_naive() >= d,
                "{tz} {d}: {got} is before the date"
            );
            assert!(
                (got - Duration::minutes(1)).with_timezone(&tz).date_naive() < d,
                "{tz} {d}: a minute earlier is already on the date"
            );
        }
    }

    #[test]
    fn a_dst_day_is_twenty_three_or_twenty_five_hours_long() {
        let spring =
            day_start_utc(date(2026, 10, 5), Sydney) - day_start_utc(date(2026, 10, 4), Sydney);
        assert_eq!(spring, Duration::hours(23));
        let fall =
            day_start_utc(date(2027, 4, 5), Sydney) - day_start_utc(date(2027, 4, 4), Sydney);
        assert_eq!(fall, Duration::hours(25));
    }

    #[test]
    fn today_in_flips_at_the_zones_midnight_not_utcs() {
        // Sydney is +11 on 2026-03-02: local midnight is 2026-03-01T13:00Z.
        assert_eq!(today_in(Sydney, utc(2026, 3, 1, 13, 0)), date(2026, 3, 2));
        assert_eq!(today_in(Sydney, utc(2026, 3, 1, 12, 59)), date(2026, 3, 1));
        // The same instant is still the 1st in New York (-5).
        assert_eq!(today_in(New_York, utc(2026, 3, 1, 13, 0)), date(2026, 3, 1));
    }

    #[test]
    fn registration_deadline_is_the_start_of_the_first_anticipation_day() {
        // first switch 2026-03-02, anticipation 3 -> 2026-02-27, 00:00 in Sydney (+11).
        assert_eq!(
            registration_deadline(&full()),
            Some(utc(2026, 2, 26, 13, 0))
        );
    }

    #[test]
    fn registration_deadline_counts_calendar_days_across_a_dst_change() {
        // switch 2026-10-06, anticipation 3 -> 2026-10-03 00:00 AEST (+10) = 10-02T14:00Z.
        // "switch minus 72 hours" would land an hour off (13:00Z), because the
        // clocks went forward on the 4th.
        let mut c = full();
        c.design.waves = vec![wave(2026, 10, 6)];
        assert_eq!(registration_deadline(&c), Some(utc(2026, 10, 2, 14, 0)));
    }

    #[test]
    fn registration_deadline_uses_the_earliest_wave_whatever_the_listing_order() {
        let mut c = full();
        c.design.waves = vec![wave(2026, 3, 16), wave(2026, 3, 2)];
        assert_eq!(registration_deadline(&c), Some(utc(2026, 2, 26, 13, 0)));
    }

    #[test]
    fn a_draft_has_no_deadline_and_no_window_end() {
        assert_eq!(registration_deadline(&draft()), None);
        assert_eq!(last_post_window_end_date(&draft()), None);
    }

    #[test]
    fn last_post_window_end_matches_the_estimators_half_open_post_window() {
        // Review Focus 3. Estimator: post = [switch + washout, switch + washout + post_days)
        // in day ordinals (num_days_from_ce). The LAST wave by date is 03-16 even
        // though it is listed first.
        let mut c = full();
        c.design.waves = vec![wave(2026, 3, 16), wave(2026, 3, 2)];
        let switch_ord = i64::from(date(2026, 3, 16).num_days_from_ce());
        let post_end_ord =
            switch_ord + i64::from(c.design.washout_days) + i64::from(c.design.post_days.unwrap());

        let end = last_post_window_end_date(&c).unwrap();
        assert_eq!(i64::from(end.num_days_from_ce()), post_end_ord);
        assert_eq!(end, date(2026, 5, 18), "2026-03-16 + 7 washout + 56 post");
        assert_eq!(
            end.pred_opt().unwrap(),
            date(2026, 5, 17),
            "the last day INSIDE the window"
        );

        // Anticipation is a pre-side band: it must not move the post window.
        c.design.anticipation_days = 30;
        assert_eq!(last_post_window_end_date(&c), Some(end));
    }

    #[test]
    fn a_switchbacks_deadline_is_the_start_of_its_first_period() {
        // First period 2026-03-02, Sydney +11 -> 2026-03-01T13:00Z.
        let mut c = switchback();
        assert_eq!(registration_deadline(&c), Some(utc(2026, 3, 1, 13, 0)));
        // The subtraction is kept for a config validate_static would refuse:
        // 2 days earlier is 2026-02-28 00:00 +11 = 02-27T13:00Z.
        c.design.anticipation_days = 2;
        assert_eq!(registration_deadline(&c), Some(utc(2026, 2, 27, 13, 0)));
    }

    #[test]
    fn last_post_window_end_for_a_switchback_is_the_day_after_the_last_period() {
        // Periods start 03-02, 03-09, 03-16, 03-23; each is 7 days, so the last
        // runs 03-23..=03-29 and the exclusive end is 03-30. The washout (2) is
        // inside every period and moves nothing.
        let mut c = switchback();
        let end = last_post_window_end_date(&c).unwrap();
        assert_eq!(end, date(2026, 3, 30));
        assert_eq!(
            end.pred_opt().unwrap(),
            date(2026, 3, 29),
            "the last day INSIDE the last period"
        );
        c.design.washout_days = 6;
        assert_eq!(last_post_window_end_date(&c), Some(end));
    }

    #[test]
    fn a_switchback_draft_has_no_deadline_and_no_window_end() {
        assert_eq!(registration_deadline(&switchback_draft()), None);
        assert_eq!(last_post_window_end_date(&switchback_draft()), None);
    }

    #[test]
    fn a_waves_design_without_its_post_window_has_no_window_end() {
        // validate_static refuses this file; the function does not invent a window.
        let mut c = full();
        c.design.post_days = None;
        assert_eq!(last_post_window_end_date(&c), None);
    }

    #[test]
    fn at_week_grain_the_windows_and_the_deadline_count_weeks() {
        let mut c = full();
        c.design.grain = crate::Grain::Week;
        c.design.pre_days = Some(8);
        c.design.post_days = Some(4);
        c.design.washout_days = 1;
        c.design.anticipation_days = 1;
        // Last switch Monday 2026-03-16, then 1 washout + 4 post weeks.
        assert_eq!(last_post_window_end_date(&c), Some(date(2026, 4, 20)));
        // First switch Monday 2026-03-02, one anticipation week earlier.
        assert_eq!(
            registration_deadline(&c),
            Some(day_start_utc(date(2026, 2, 23), c.design.timezone))
        );
    }

    #[test]
    fn at_month_grain_a_switchback_ends_a_month_after_its_last_period() {
        let mut c = switchback();
        c.design.grain = crate::Grain::Month;
        let sb = c.design.switchback.as_mut().unwrap();
        sb.period_days = 1;
        for (p, m) in sb.periods.iter_mut().zip(3..) {
            p.from = date(2026, m, 1).into();
        }
        assert_eq!(last_post_window_end_date(&c), Some(date(2026, 7, 1)));
    }

    #[test]
    fn at_hour_grain_the_close_is_the_midnight_after_the_last_post_hour() {
        let mut c = full();
        c.design.grain = crate::Grain::Hour;
        c.design.washout_days = 6;
        c.design.post_days = Some(30);
        c.design.anticipation_days = 3;
        // Last switch 2026-03-16 00:00; post ends 03-17 12:00, so the run closes at 03-18.
        assert_eq!(last_post_window_end_date(&c), Some(date(2026, 3, 18)));
        c.design.post_days = Some(42);
        assert_eq!(
            last_post_window_end_date(&c),
            Some(date(2026, 3, 18)),
            "ends on midnight exactly"
        );
        // First switch 2026-03-02 00:00 local, three hours before it.
        let tz = c.design.timezone;
        let expected = tz
            .from_local_datetime(&date(2026, 3, 1).and_hms_opt(21, 0, 0).unwrap())
            .earliest()
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(registration_deadline(&c), Some(expected));
    }
}
