//! The series the chat `detect_anomalies` tool scores.
//!
//! The tool sends the store raw `(bucket, value)` rows for a history window
//! plus the period the user asked about. Three things must hold before the
//! detector sees them, and each one used to fail:
//!
//! - **Every bucket parses.** The rows carry the warehouse's own bucket text,
//!   such as `2026-06-01 00:00:00` with no offset. The old parser wanted an
//!   RFC 3339 offset, dropped every such row without a word, and the tool
//!   reported "0 observations" for every series. This takes the formats the
//!   scheduled scan takes ([`parse_flexible_date`]).
//! - **The series is evenly spaced.** A bucket with no row (a day with no
//!   orders) is filled with an imputed `0.0`, as the scheduled scan does, so
//!   the seasonal index stays aligned. The detector never scores an imputed
//!   bucket (see `detect`), so a missing row is reported as missing instead:
//!   [`ToolSeries::missing`].
//! - **The whole asked period is scored.** Every bucket from the period's
//!   start to its end is in the test window, and the history before it is the
//!   training window. Before, only the last bucket was scored, so "anomalies
//!   in June" checked June 30 alone. A bucket belongs to the period when any
//!   of its days does, so a period that starts mid-week still scores that week.
//! - **Only finished buckets count.** A bucket that has not ended by today
//!   (this week, this month) is left out, as the scheduled scan leaves out the
//!   current incomplete period: a part-week total would read as a collapse.

use chrono::{Duration, Months, NaiveDate};

use crate::config::{Granularity, Sensitivity};
use crate::detect::{DetectError, DetectInputs, DetectedAnomaly, Observation, detect};
use crate::metric_tree_series::parse_flexible_date;
use crate::service::{advance_period, default_test_window, resolve_local_midnight};

/// A series ready for `detect`, and what the period holds.
#[derive(Debug)]
pub(crate) struct ToolSeries {
    /// History, then the asked period, evenly spaced.
    pub series: Vec<Observation>,
    /// How many trailing buckets of `series` are the asked period.
    pub test_window: usize,
    /// Buckets inside the asked period with no row, as `YYYY-MM-DD`.
    pub missing: Vec<String>,
    /// Rows inside the asked period left out because their bucket has not
    /// ended by today, such as this month's running total.
    pub unfinished: usize,
}

impl ToolSeries {
    /// Buckets before the asked period: what the detector trains on.
    pub fn history_len(&self) -> usize {
        self.series.len() - self.test_window
    }
}

/// The most buckets one call scores. Each chunk is one detector fit, so this
/// bounds the work a model-chosen period can ask for (about 15 fits for a
/// quarter of days).
const MAX_SCORED: usize = 100;

/// What scoring a [`ToolSeries`] came to.
#[derive(Debug)]
pub(crate) enum Scored {
    /// The buckets the detector flagged, across the whole period.
    Flagged(Vec<DetectedAnomaly>),
    /// The period could not be scored; the message says why.
    NotEnough(String),
}

/// Score every bucket of the asked period, refusing first when the period
/// has no rows or fewer than `min_history` measured buckets precede it.
/// `min_history` must be at least `detect`'s own floor, so no chunk fails it.
pub(crate) fn score(
    s: &ToolSeries,
    seasonal: &[usize],
    min_history: usize,
    granularity: &str,
    period: &(String, String),
) -> Result<Scored, DetectError> {
    let (history, period_rows) = s.series.split_at(s.history_len());
    let measured_history = history.iter().filter(|o| !o.imputed).count();
    if period_rows.is_empty() && s.unfinished > 0 {
        let why = format!(
            "Not finished: the buckets from {} to {} have not ended yet, so their partial \
             totals are not scored. Ask about a finished period.",
            period.0, period.1
        );
        return Ok(Scored::NotEnough(why));
    }
    if period_rows.iter().all(|o| o.imputed) {
        let why = format!("Not enough data: no rows from {} to {}", period.0, period.1);
        return Ok(Scored::NotEnough(why));
    }
    if s.test_window > MAX_SCORED {
        let why = format!(
            "Period too long: {} buckets from {} to {}, at most {MAX_SCORED}. Ask for a \
             shorter period.",
            s.test_window, period.0, period.1
        );
        return Ok(Scored::NotEnough(why));
    }
    if measured_history < min_history {
        let why = format!(
            "Not enough data: {measured_history} observations before {}, need at least \
             {min_history}",
            period.0
        );
        return Ok(Scored::NotEnough(why));
    }
    // A few buckets at a time, each chunk against all the history before it,
    // as the scheduled scan does: one forecast over a whole month would widen
    // its interval until nothing flags.
    let mut flagged = Vec::new();
    for (end, window) in test_chunks(s.series.len(), s.test_window, granularity) {
        flagged.extend(detect(DetectInputs {
            series: &s.series[..end],
            seasonal_periods: seasonal.to_vec(),
            test_window: window,
            sensitivity: Sensitivity::Medium,
            interval_level: 0.95,
            // A caller-supplied series has no segment to look events up by.
            continuation: None,
        })?);
    }
    Ok(Scored::Flagged(flagged))
}

/// The `(end, window)` slices that score the last `test_window` buckets of a
/// `len`-bucket series: `detect` on `series[..end]` with that `window`. Each
/// chunk is at most the scheduled scan's test window for the granularity (a
/// week of days, one week, one month), so no forecast runs further ahead than
/// a scan's does.
pub(crate) fn test_chunks(
    len: usize,
    test_window: usize,
    granularity: &str,
) -> Vec<(usize, usize)> {
    let step = fill_step(granularity).map_or(1, default_test_window).max(1);
    let mut out = Vec::new();
    let mut start = len - test_window.min(len);
    while start < len {
        let end = (start + step).min(len);
        out.push((end, end - start));
        start = end;
    }
    out
}

/// The calendar step for gap-filling. A granularity the scheduled scan does
/// not fill (such as `quarter`) gets none, and its rows are scored as given.
fn fill_step(granularity: &str) -> Option<Granularity> {
    match granularity {
        "day" => Some(Granularity::Day),
        "week" => Some(Granularity::Week),
        "month" => Some(Granularity::Month),
        _ => None,
    }
}

/// The day after a bucket labelled `start` ends. A label is the bucket's
/// first day: a day, the warehouse's week start, or the 1st of a month or
/// quarter.
fn bucket_end(start: NaiveDate, granularity: &str) -> NaiveDate {
    let months = |n| start.checked_add_months(Months::new(n));
    match granularity {
        "week" => start + Duration::weeks(1),
        "month" => months(1).unwrap_or(start + Duration::days(31)),
        "quarter" => months(3).unwrap_or(start + Duration::days(92)),
        _ => start + Duration::days(1),
    }
}

/// Build the scored series from the tool's rows. `period` is the asked
/// `(start, end)`, both inclusive. A bound that does not parse leaves the
/// old behaviour for that bound: no start means only the last bucket is
/// scored, and no end means the series stops at the last row. A bucket that
/// has not ended by `today` is left out: it is neither scored nor missing.
pub(crate) fn prepare(
    rows: &[(String, f64)],
    granularity: &str,
    period: &(String, String),
    today: NaiveDate,
) -> ToolSeries {
    let mut dated: Vec<(NaiveDate, f64)> = rows
        .iter()
        .filter_map(|(ts, value)| parse_flexible_date(ts).map(|d| (d, *value)))
        .collect();
    dated.sort_by_key(|(d, _)| *d);
    dated.dedup_by_key(|(d, _)| *d);
    let start = parse_flexible_date(&period.0);
    let unfinished = dated
        .iter()
        .map(|(d, _)| bucket_end(*d, granularity))
        .filter(|end| *end > today && start.is_none_or(|s| *end > s))
        .count();
    dated.retain(|(d, _)| bucket_end(*d, granularity) <= today);

    // The history fetch starts a whole number of units before `start`, which
    // need not be a bucket boundary for weeks, months or quarters, so the
    // earliest bucket may hold only part of its days. Drop it.
    let first_is_partial_history = granularity != "day"
        && start
            .zip(dated.first())
            .is_some_and(|(s, (d, _))| bucket_end(*d, granularity) <= s);
    if first_is_partial_history {
        dated.remove(0);
    }
    let end = parse_flexible_date(&period.1);
    let filled = match fill_step(granularity) {
        Some(step) => fill(&dated, step, end, today),
        None => dated.into_iter().map(|(d, v)| (d, v, false)).collect(),
    };

    // A bucket is in the period when any of its days is: one that starts
    // before `start` but ends after it still holds part of the period.
    let test_window = match start {
        Some(start) => filled
            .iter()
            .filter(|(d, _, _)| bucket_end(*d, granularity) > start)
            .count(),
        None => filled.len().min(1),
    };
    let missing = filled
        .iter()
        .rev()
        .take(test_window)
        .filter(|(_, _, imputed)| *imputed)
        .map(|(d, _, _)| d.format("%Y-%m-%d").to_string())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let series = filled
        .into_iter()
        .map(|(d, value, imputed)| {
            let timestamp = resolve_local_midnight(d, chrono_tz::UTC);
            if imputed {
                Observation::filled(timestamp)
            } else {
                Observation::measured(timestamp, value)
            }
        })
        .collect();
    ToolSeries {
        series,
        test_window,
        missing,
        unfinished,
    }
}

/// Fill every bucket from the first row to `end` (or the last row), marking
/// the invented ones, and stop at the last bucket that has ended by `today`.
/// `end` matters: a period whose last days have no rows must still list them
/// as missing.
fn fill(
    dated: &[(NaiveDate, f64)],
    step: Granularity,
    end: Option<NaiveDate>,
    today: NaiveDate,
) -> Vec<(NaiveDate, f64, bool)> {
    let (Some(first), Some(last)) = (dated.first(), dated.last()) else {
        return Vec::new();
    };
    let stop = end.map_or(last.0, |e| e.max(last.0));
    let mut rows = dated.iter().peekable();
    let mut out = Vec::new();
    let mut cur = first.0;
    while cur <= stop && advance_period(cur, step) <= today {
        while rows.peek().is_some_and(|(d, _)| *d < cur) {
            rows.next();
        }
        match rows.peek() {
            Some((d, v)) if *d == cur => out.push((cur, *v, false)),
            _ => out.push((cur, 0.0, true)),
        }
        cur = advance_period(cur, step);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed "today" well after every test period.
    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 28).unwrap()
    }

    fn june() -> (String, String) {
        ("2026-06-01".to_string(), "2026-06-30".to_string())
    }

    /// A day of rows from `from` for `days` days, in the warehouse's own
    /// bucket text, skipping the dates in `skip`.
    fn daily(from: &str, days: i64, skip: &[&str]) -> Vec<(String, f64)> {
        let start = NaiveDate::parse_from_str(from, "%Y-%m-%d").unwrap();
        (0..days)
            .map(|i| start + chrono::Duration::days(i))
            .filter(|d| !skip.contains(&d.format("%Y-%m-%d").to_string().as_str()))
            .map(|d| (format!("{} 00:00:00", d.format("%Y-%m-%d")), 100.0))
            .collect()
    }

    #[test]
    fn warehouse_bucket_text_without_an_offset_is_kept() {
        let rows = daily("2026-03-01", 122, &[]);
        let s = prepare(&rows, "day", &june(), today());
        assert_eq!(s.series.len(), 122, "no bucket may be dropped");
        assert!(s.series.iter().all(|o| !o.imputed));
    }

    #[test]
    fn every_day_of_the_asked_period_is_in_the_test_window() {
        let rows = daily("2026-03-01", 122, &[]);
        let s = prepare(&rows, "day", &june(), today());
        assert_eq!(s.test_window, 30);
        assert_eq!(s.history_len(), 92);
    }

    #[test]
    fn a_day_with_no_row_is_filled_and_reported_missing() {
        let rows = daily("2026-03-01", 122, &["2026-06-16", "2026-04-16"]);
        let s = prepare(&rows, "day", &june(), today());
        assert_eq!(s.series.len(), 122, "the gaps are filled, not skipped");
        assert_eq!(s.test_window, 30);
        assert_eq!(s.missing, vec!["2026-06-16".to_string()]);
        let imputed = s.series.iter().filter(|o| o.imputed).count();
        assert_eq!(imputed, 2, "the April gap is filled too, but it is history");
    }

    #[test]
    fn missing_days_at_the_end_of_the_period_are_reported() {
        let rows = daily("2026-03-01", 118, &[]); // through 2026-06-26
        let s = prepare(&rows, "day", &june(), today());
        assert_eq!(s.test_window, 30);
        assert_eq!(
            s.missing,
            ["2026-06-27", "2026-06-28", "2026-06-29", "2026-06-30"]
        );
    }

    #[test]
    fn an_unreadable_period_start_scores_only_the_last_bucket() {
        let rows = daily("2026-03-01", 122, &[]);
        let period = ("June".to_string(), "2026-06-30".to_string());
        assert_eq!(prepare(&rows, "day", &period, today()).test_window, 1);
    }

    #[test]
    fn rfc3339_and_plain_dates_parse_too() {
        let rows = vec![
            ("2026-06-01".to_string(), 1.0),
            ("2026-06-02T00:00:00Z".to_string(), 2.0),
            ("2026-06-03 00:00:00".to_string(), 3.0),
        ];
        let period = ("2026-06-01".to_string(), "2026-06-03".to_string());
        let s = prepare(&rows, "day", &period, today());
        let values: Vec<f64> = s.series.iter().map(|o| o.value).collect();
        assert_eq!(values, [1.0, 2.0, 3.0]);
    }

    /// The point of scoring the whole period: a spike in the middle of
    /// June is found. Before, only June 30 was scored, and no row parsed.
    #[test]
    fn a_spike_mid_period_is_flagged_through_the_chunks() {
        let by_weekday = [1500.0, 1450.0, 1520.0, 1480.0, 1600.0, 1900.0, 2300.0];
        let start = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
        let rows: Vec<(String, f64)> = (0..122_usize)
            .map(|i| {
                let d = start + chrono::Duration::days(i as i64);
                // A little deterministic wobble so sigma is not degenerate.
                let jitter = ((i * 37) % 11) as f64 - 5.0;
                let mut value = by_weekday[i % 7] + jitter * 4.0;
                if d == NaiveDate::from_ymd_opt(2026, 6, 16).unwrap() {
                    value *= 4.0;
                }
                (format!("{} 00:00:00", d.format("%Y-%m-%d")), value)
            })
            .collect();
        let s = prepare(&rows, "day", &june(), today());
        let Scored::Flagged(flagged) = score(&s, &[7], 56, "day", &june()).expect("detects") else {
            panic!("92 days of history is enough");
        };
        let spike = resolve_local_midnight(
            NaiveDate::from_ymd_opt(2026, 6, 16).unwrap(),
            chrono_tz::UTC,
        );
        assert!(
            flagged.iter().any(|a| a.timestamp == spike),
            "June 16 must be flagged: {:?}",
            flagged.iter().map(|a| a.timestamp).collect::<Vec<_>>()
        );
    }

    #[test]
    fn too_little_history_or_an_empty_period_is_not_scored() {
        let short = prepare(&daily("2026-05-20", 42, &[]), "day", &june(), today());
        let Ok(Scored::NotEnough(why)) = score(&short, &[7], 56, "day", &june()) else {
            panic!("12 days of history is not enough");
        };
        assert!(why.contains("12 observations before 2026-06-01"), "{why}");

        let before = ("2026-08-01".to_string(), "2026-08-31".to_string());
        let empty = prepare(&daily("2026-03-01", 122, &[]), "day", &before, today());
        let Ok(Scored::NotEnough(why)) = score(&empty, &[7], 56, "day", &before) else {
            panic!("August has no rows");
        };
        assert!(why.contains("no rows"), "{why}");

        let year = ("2025-07-01".to_string(), "2026-06-30".to_string());
        let long = prepare(&daily("2025-01-01", 546, &[]), "day", &year, today());
        let Ok(Scored::NotEnough(why)) = score(&long, &[7], 56, "day", &year) else {
            panic!("a year of days is over the cap");
        };
        assert!(why.contains("Period too long: 365 buckets"), "{why}");
    }

    #[test]
    fn days_from_today_on_are_not_called_missing() {
        let rows = daily("2026-03-01", 122, &[]); // through 2026-06-30
        let s = prepare(
            &rows,
            "day",
            &june(),
            NaiveDate::from_ymd_opt(2026, 6, 28).unwrap(),
        );
        assert!(s.missing.is_empty(), "{:?}", s.missing);
        assert_eq!(s.test_window, 27, "June 28 and later have not finished");
    }

    /// Weekly rows labelled on Mondays, from `from` for `weeks` weeks.
    fn weekly(from: &str, weeks: i64) -> Vec<(String, f64)> {
        let start = NaiveDate::parse_from_str(from, "%Y-%m-%d").unwrap();
        (0..weeks)
            .map(|i| {
                let d = start + Duration::weeks(i);
                (format!("{} 00:00:00", d.format("%Y-%m-%d")), 10.0)
            })
            .collect()
    }

    #[test]
    fn a_period_starting_mid_week_scores_the_week_that_holds_its_first_day() {
        // July 1, 2026 is a Wednesday; its week is labelled Monday June 29.
        let rows = weekly("2025-09-01", 48); // through the week of 2026-07-27
        let july = ("2026-07-01".to_string(), "2026-07-31".to_string());
        let s = prepare(&rows, "week", &july, today());
        assert_eq!(s.test_window, 5, "June 29, July 6, 13, 20 and 27");
    }

    #[test]
    fn a_mid_month_period_scores_its_month() {
        // Monthly rows from January 2024 to June 2026.
        let rows: Vec<(String, f64)> = (0..30)
            .map(|i| (format!("{}-{:02}-01", 2024 + i / 12, i % 12 + 1), 5.0))
            .collect();
        let late_june = ("2026-06-15".to_string(), "2026-06-30".to_string());
        let s = prepare(&rows, "month", &late_june, today());
        assert_eq!(s.test_window, 1, "the June bucket holds June 15-30");
        assert_eq!(s.series.len(), 29, "the first month is dropped as partial");
    }

    #[test]
    fn the_week_still_running_is_neither_scored_nor_missing() {
        // Today is Saturday 2026-11-28; the week of Monday 11-23 is not over.
        let rows = weekly("2025-12-01", 52); // through the week of 2026-11-23
        let november = ("2026-11-01".to_string(), "2026-11-30".to_string());
        let s = prepare(&rows, "week", &november, today());
        assert!(s.missing.is_empty(), "{:?}", s.missing);
        // The weeks of Oct 26 (it holds Nov 1), Nov 2, 9 and 16; not Nov 23.
        assert_eq!(s.test_window, 4);
    }

    #[test]
    fn a_period_still_running_says_so_instead_of_no_rows() {
        // Monthly rows through November 2026; today is 2026-11-28.
        let rows: Vec<(String, f64)> = (0..35)
            .map(|i| (format!("{}-{:02}-01", 2024 + i / 12, i % 12 + 1), 5.0))
            .collect();
        let november = ("2026-11-01".to_string(), "2026-11-30".to_string());
        let s = prepare(&rows, "month", &november, today());
        assert_eq!((s.test_window, s.unfinished), (0, 1));
        let Ok(Scored::NotEnough(why)) = score(&s, &[12], 24, "month", &november) else {
            panic!("November has not ended");
        };
        assert!(why.starts_with("Not finished"), "{why}");
    }

    #[test]
    fn a_partial_first_history_bucket_is_dropped() {
        // The fetch began on a Wednesday, so its first week is partial.
        let mut rows = vec![("2025-08-25 00:00:00".to_string(), 3.0)];
        rows.extend(weekly("2025-09-01", 48));
        let july = ("2026-07-01".to_string(), "2026-07-31".to_string());
        let s = prepare(&rows, "week", &july, today());
        assert_eq!(s.series.len(), 48, "the 2025-08-25 bucket is gone");
    }

    #[test]
    fn a_month_of_days_is_scored_a_week_at_a_time() {
        // 92 days of history, then June's 30.
        assert_eq!(
            test_chunks(122, 30, "day"),
            [(99, 7), (106, 7), (113, 7), (120, 7), (122, 2)]
        );
        assert_eq!(test_chunks(40, 3, "month"), [(38, 1), (39, 1), (40, 1)]);
        assert!(test_chunks(40, 0, "day").is_empty());
    }

    #[test]
    fn a_granularity_the_scan_does_not_fill_is_scored_as_given() {
        // A gap at 2026-04-01 stays a gap: quarters are not filled. The first
        // row is dropped as possibly partial history.
        let rows = vec![
            ("2025-10-01".to_string(), 1.0),
            ("2026-01-01".to_string(), 1.0),
            ("2026-07-01".to_string(), 2.0),
        ];
        let period = ("2026-07-01".to_string(), "2026-09-30".to_string());
        let s = prepare(&rows, "quarter", &period, today());
        assert_eq!(s.series.len(), 2);
        assert_eq!(s.test_window, 1);
        assert!(s.missing.is_empty());
    }
}
