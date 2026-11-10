//! `group_by` fan-out: pair two operands' per-group values on their key and
//! name the groups outside tolerance. Pure — no I/O — so the join, the
//! one-sided rule and the ordering are unit-testable without a warehouse.
//!
//! A `group_by` the runner cannot execute degrades only its own check
//! ([`super::config::ReconcileCheck::group_by_plan`]); it never fails the file.
//!
//! Rolling deploys: binaries before this one type `group_by` as a single
//! string, so a list fails the whole file's parse on an instance still running
//! one. A list goes into a workspace only once this release serves everywhere.
//!
//! The fan-out is *detail*. A check's status and totals come from the ungrouped
//! comparison exactly as before; this only answers "which segments?" when a
//! total goes red (or when segments offset each other inside a green one).

use std::collections::BTreeMap;

use serde::Serialize;

use super::compare::{Tolerance, breach};

/// The most disputed segments a verdict carries. Sorted by |delta| first, so
/// the cap drops the smallest disputes; `groups_disputed` keeps the full count.
pub const MAX_REPORTED_SEGMENTS: usize = 20;

/// One operand's contribution to a grouped comparison.
#[derive(Debug, Clone)]
pub enum GroupedSide {
    /// One value per group. Each key holds the group's values in `group_by`
    /// order. Rows that share a key (after [`normalize_key_value`]) are summed.
    Rows(Vec<(Vec<String>, f64)>),
    /// A `constant` operand: the same value for every group the other side
    /// returned. A constant names no groups of its own, so it is never the
    /// "missing" side.
    Broadcast(f64),
}

/// The per-segment detail attached to a grouped check's verdict.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SegmentReport {
    /// The key names, in order; each segment's `key` lines up with this.
    pub group_by: Vec<String>,
    /// Distinct groups seen on either side.
    pub groups_compared: usize,
    /// Groups outside tolerance — the full count, before the cap.
    pub groups_disputed: usize,
    /// The disputed groups, largest |delta| first, at most
    /// [`MAX_REPORTED_SEGMENTS`].
    pub disputed: Vec<SegmentDrift>,
    /// Set when the grouped evaluation itself failed. The verdict's status is
    /// still the ungrouped one; this only says the detail is unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One group outside tolerance.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SegmentDrift {
    /// The group's key values, in `group_by` order.
    pub key: Vec<String>,
    pub actual: f64,
    pub expected: f64,
    /// Signed `actual - expected`.
    pub delta: f64,
    /// Percent drift relative to `expected`, as on the verdict (`0.0` when
    /// undefined, i.e. `expected` is 0).
    pub pct_diff: f64,
    /// Which side had no row for this group (read as 0). Absent when both did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missing: Option<MissingSide>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingSide {
    Actual,
    Expected,
}

/// Join both sides on the group key, apply `tol` per group, and report the
/// disputed groups. A group present on only one side is compared against 0.
pub fn fan_out(
    group_by: &[String],
    actual: GroupedSide,
    expected: GroupedSide,
    tol: &Tolerance,
) -> SegmentReport {
    let actual = Side::from(actual);
    let expected = Side::from(expected);

    let mut keys: Vec<&Vec<String>> = actual.keys().chain(expected.keys()).collect();
    keys.sort();
    keys.dedup();

    let mut disputed = Vec::new();
    for key in &keys {
        let (a, a_missing) = actual.value(key);
        let (e, e_missing) = expected.value(key);
        let (_, pct_diff, breached) = breach(a, e, tol);
        if !breached {
            continue;
        }
        disputed.push(SegmentDrift {
            key: (*key).clone(),
            actual: a,
            expected: e,
            delta: a - e,
            pct_diff: if pct_diff.is_finite() { pct_diff } else { 0.0 },
            missing: if a_missing {
                Some(MissingSide::Actual)
            } else if e_missing {
                Some(MissingSide::Expected)
            } else {
                None
            },
        });
    }
    // Largest dispute first; the key (already sorted) breaks ties so the list
    // is stable across runs.
    disputed.sort_by(|x, y| y.delta.abs().total_cmp(&x.delta.abs()));
    let groups_disputed = disputed.len();
    disputed.truncate(MAX_REPORTED_SEGMENTS);

    SegmentReport {
        group_by: group_by.to_vec(),
        groups_compared: keys.len(),
        groups_disputed,
        disputed,
        error: None,
    }
}

/// The detail for a grouped evaluation that failed (a query error on either
/// side). The check's status is unaffected.
pub fn segment_error(group_by: &[String], error: String) -> SegmentReport {
    SegmentReport {
        group_by: group_by.to_vec(),
        groups_compared: 0,
        groups_disputed: 0,
        disputed: Vec::new(),
        error: Some(error),
    }
}

/// Canonical text for a key value, so the same group read from two engines
/// joins: a midnight timestamp collapses to its date (`2026-07-01T00:00:00Z`
/// and `2026-07-01` are one day) and a whole decimal drops its fraction
/// (`12.0` → `12`). Anything else is only trimmed — an integer-looking string
/// such as `007` keeps its leading zeros, because both sides read it the same.
pub fn normalize_key_value(raw: &str) -> String {
    let s = raw.trim();
    if let Some(date) = midnight_date(s) {
        return date.to_string();
    }
    if s.contains('.')
        && let Ok(n) = s.parse::<f64>()
        && n.is_finite()
        && n.fract() == 0.0
        && n.abs() < 1e15
    {
        return format!("{n:.0}");
    }
    s.to_string()
}

/// `Some("YYYY-MM-DD")` when `s` is that date alone or that date at midnight
/// UTC, in the spellings warehouses render a DATE cast to a timestamp.
fn midnight_date(s: &str) -> Option<&str> {
    let date = s.get(..10)?;
    if chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err() {
        return None;
    }
    let rest = &s[10..];
    if rest.is_empty() {
        return Some(date);
    }
    let rest = rest.strip_prefix(['T', ' '])?.strip_prefix("00:00:00")?;
    let rest = match rest.strip_prefix('.') {
        Some(frac) => frac.trim_start_matches('0'),
        None => rest,
    };
    matches!(rest, "" | "Z" | "+00" | "+00:00" | "+0000" | " UTC").then_some(date)
}

/// A side, keyed and summed.
enum Side {
    Rows(BTreeMap<Vec<String>, f64>),
    Broadcast(f64),
}

impl From<GroupedSide> for Side {
    fn from(side: GroupedSide) -> Self {
        match side {
            GroupedSide::Broadcast(n) => Side::Broadcast(n),
            GroupedSide::Rows(rows) => {
                let mut map = BTreeMap::new();
                for (key, value) in rows {
                    let key: Vec<String> = key.iter().map(|k| normalize_key_value(k)).collect();
                    *map.entry(key).or_insert(0.0) += value;
                }
                Side::Rows(map)
            }
        }
    }
}

impl Side {
    fn keys(&self) -> Box<dyn Iterator<Item = &Vec<String>> + '_> {
        match self {
            Side::Rows(map) => Box::new(map.keys()),
            Side::Broadcast(_) => Box::new(std::iter::empty()),
        }
    }

    /// The group's value and whether this side lacked it (read as 0).
    fn value(&self, key: &[String]) -> (f64, bool) {
        match self {
            Side::Broadcast(n) => (*n, false),
            Side::Rows(map) => match map.get(key) {
                Some(v) => (*v, false),
                None => (0.0, true),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::compare::Combinator;
    use super::*;

    fn tol(abs: f64, pct: f64) -> Tolerance {
        Tolerance {
            abs,
            pct,
            combinator: Combinator::And,
        }
    }

    fn keys(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn rows(rows: &[(&[&str], f64)]) -> GroupedSide {
        GroupedSide::Rows(rows.iter().map(|(k, v)| (keys(k), *v)).collect())
    }

    #[test]
    fn grouped_within_tolerance_disputes_nothing() {
        let r = fan_out(
            &keys(&["store", "date"]),
            rows(&[(&["a", "2026-07-01"], 100.0), (&["b", "2026-07-01"], 50.0)]),
            rows(&[(&["a", "2026-07-01"], 100.4), (&["b", "2026-07-01"], 50.0)]),
            &tol(1.0, 0.5),
        );
        assert_eq!(r.groups_compared, 2);
        assert_eq!(r.groups_disputed, 0);
        assert!(r.disputed.is_empty());
        assert_eq!(r.error, None);
    }

    #[test]
    fn grouped_failure_names_the_groups_largest_first() {
        let r = fan_out(
            &keys(&["vendor", "company", "month"]),
            rows(&[
                (&["olo", "pokehouse", "2026-05"], 1_000.0),
                (&["olo", "pokehouse", "2026-06"], 148.06),
                (&["grubhub", "pokehouse", "2026-06"], 500.0),
            ]),
            rows(&[
                (&["olo", "pokehouse", "2026-05"], 400.0),
                (&["olo", "pokehouse", "2026-06"], 123.89),
                (&["grubhub", "pokehouse", "2026-06"], 500.0),
            ]),
            &tol(0.5, 0.0),
        );
        assert_eq!(r.group_by, keys(&["vendor", "company", "month"]));
        assert_eq!(r.groups_compared, 3);
        assert_eq!(r.groups_disputed, 2);
        assert_eq!(r.disputed[0].key, keys(&["olo", "pokehouse", "2026-05"]));
        assert_eq!(r.disputed[0].actual, 1_000.0);
        assert_eq!(r.disputed[0].expected, 400.0);
        assert_eq!(r.disputed[0].delta, 600.0);
        assert_eq!(r.disputed[0].pct_diff, 150.0);
        assert_eq!(r.disputed[0].missing, None);
        assert_eq!(r.disputed[1].key, keys(&["olo", "pokehouse", "2026-06"]));
        assert!((r.disputed[1].delta - 24.17).abs() < 1e-9);
    }

    #[test]
    fn a_group_on_one_side_reads_the_other_as_zero() {
        let r = fan_out(
            &keys(&["store"]),
            rows(&[(&["a"], 100.0), (&["only_actual"], 30.0)]),
            rows(&[(&["a"], 100.0), (&["only_expected"], 20.0)]),
            &tol(1.0, 0.5),
        );
        assert_eq!(r.groups_compared, 3);
        assert_eq!(r.groups_disputed, 2);
        let actual_only = &r.disputed[0];
        assert_eq!(actual_only.key, keys(&["only_actual"]));
        assert_eq!((actual_only.actual, actual_only.expected), (30.0, 0.0));
        assert_eq!(actual_only.missing, Some(MissingSide::Expected));
        // expected == 0 → pct undefined, stored as 0.0 like the verdict.
        assert_eq!(actual_only.pct_diff, 0.0);
        let expected_only = &r.disputed[1];
        assert_eq!(expected_only.key, keys(&["only_expected"]));
        assert_eq!((expected_only.actual, expected_only.expected), (0.0, 20.0));
        assert_eq!(expected_only.delta, -20.0);
        assert_eq!(expected_only.missing, Some(MissingSide::Actual));
    }

    #[test]
    fn a_constant_applies_to_every_group_and_is_never_missing() {
        let r = fan_out(
            &keys(&["key"]),
            rows(&[(&["clean"], 0.0), (&["drifted"], -12.5)]),
            GroupedSide::Broadcast(0.0),
            &tol(0.5, 0.0),
        );
        assert_eq!(r.groups_compared, 2);
        assert_eq!(r.groups_disputed, 1);
        assert_eq!(r.disputed[0].key, keys(&["drifted"]));
        assert_eq!(r.disputed[0].delta, -12.5);
        assert_eq!(r.disputed[0].missing, None);
    }

    #[test]
    fn caps_the_list_but_keeps_the_full_count() {
        let actual: Vec<(Vec<String>, f64)> = (0..30)
            .map(|i| (vec![format!("g{i:02}")], f64::from(i) + 10.0))
            .collect();
        let r = fan_out(
            &keys(&["g"]),
            GroupedSide::Rows(actual),
            GroupedSide::Broadcast(0.0),
            &tol(1.0, 0.0),
        );
        assert_eq!(r.groups_disputed, 30);
        assert_eq!(r.disputed.len(), MAX_REPORTED_SEGMENTS);
        assert_eq!(r.disputed[0].key, keys(&["g29"]));
        assert_eq!(r.disputed[19].key, keys(&["g10"]));
    }

    #[test]
    fn keys_join_across_engine_spellings_and_duplicates_sum() {
        let r = fan_out(
            &keys(&["store", "date"]),
            rows(&[
                (&["7", "2026-07-01T00:00:00Z"], 60.0),
                (&[" 7 ", "2026-07-01 00:00:00.000"], 40.0),
            ]),
            rows(&[(&["7.0", "2026-07-01"], 100.0)]),
            &tol(0.0, 0.0),
        );
        assert_eq!(r.groups_compared, 1);
        assert_eq!(r.groups_disputed, 0);
    }

    #[test]
    fn normalize_leaves_real_values_alone() {
        assert_eq!(normalize_key_value("007"), "007");
        assert_eq!(normalize_key_value("12.5"), "12.5");
        assert_eq!(normalize_key_value("2026-07"), "2026-07");
        assert_eq!(
            normalize_key_value("2026-07-01T13:00:00Z"),
            "2026-07-01T13:00:00Z"
        );
        assert_eq!(normalize_key_value("2026-07-01+00:00"), "2026-07-01+00:00");
    }
}
