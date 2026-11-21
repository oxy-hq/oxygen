//! Pure grading functions for BIRD-style text-to-SQL evaluation: execution
//! accuracy (EX) and Soft-F1, over canonicalized result cells.
//!
//! `soft_f1` is a port of `calculate_f1_score` / `calculate_row_match` from
//! bird-bench/mini_dev `evaluation_f1.py`.

use std::collections::HashSet;

use agentic_core::result::TypedValue;
use chrono::{DateTime, NaiveDate, SecondsFormat};

/// One result cell, canonicalized so equal values from different engines compare equal.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Cell {
    Null,
    Bool(bool),
    /// Decimal string with no trailing zeros; integral values print as integers.
    Num(String),
    Text(String),
    Bytes(Vec<u8>),
}

pub(super) fn canonical(v: &TypedValue) -> Cell {
    match v {
        TypedValue::Null => Cell::Null,
        TypedValue::Bool(b) => Cell::Bool(*b),
        TypedValue::Int32(i) => Cell::Num(i.to_string()),
        TypedValue::Int64(i) => Cell::Num(i.to_string()),
        TypedValue::Float64(f) => canonical_float(*f),
        TypedValue::Decimal(s) => canonical_decimal(s),
        TypedValue::Text(s) => Cell::Text(s.clone()),
        TypedValue::Bytes(b) => Cell::Bytes(b.clone()),
        TypedValue::Date(days) => Cell::Text(format_date(*days)),
        TypedValue::Timestamp(us) => Cell::Text(format_timestamp(*us)),
        TypedValue::Json(j) => Cell::Text(j.to_string()),
    }
}

fn canonical_float(f: f64) -> Cell {
    if f.is_nan() {
        Cell::Text("NaN".into())
    } else if f.is_infinite() {
        Cell::Text(if f > 0.0 { "inf" } else { "-inf" }.into())
    } else {
        // `Display` for f64 is the shortest round-trip form, never exponent notation.
        Cell::Num(normalize_decimal(&f.to_string()))
    }
}

fn canonical_decimal(s: &str) -> Cell {
    let t = s.trim();
    let plain = t
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+'));
    if plain && t.chars().any(|c| c.is_ascii_digit()) {
        return Cell::Num(normalize_decimal(t));
    }
    // Exponent notation etc.: go through f64, or keep it as text.
    match t.parse::<f64>() {
        Ok(f) => canonical_float(f),
        Err(_) => Cell::Text(s.to_string()),
    }
}

/// Strip a leading `+`, trailing fractional zeros, and a lone `.`; map `-0` to `0`.
fn normalize_decimal(s: &str) -> String {
    let s = s.strip_prefix('+').unwrap_or(s);
    let (neg, body) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let mut out = if body.contains('.') {
        body.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        body.to_string()
    };
    if out.is_empty() || out.starts_with('.') {
        out.insert(0, '0');
    }
    if neg && out != "0" {
        out.insert(0, '-');
    }
    out
}

fn format_date(days: i32) -> String {
    // 719_163 = days from 0001-01-01 (CE day 1) to 1970-01-01.
    NaiveDate::from_num_days_from_ce_opt(days.saturating_add(719_163))
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| format!("date({days})"))
}

fn format_timestamp(us: i64) -> String {
    DateTime::from_timestamp_micros(us)
        .map(|t| t.to_rfc3339_opts(SecondsFormat::AutoSi, true))
        .unwrap_or_else(|| format!("timestamp({us})"))
}

/// Execution accuracy: row sets are equal, ignoring row order and duplicates.
/// Column order matters.
pub(super) fn execution_match(pred: &[Vec<Cell>], gold: &[Vec<Cell>]) -> bool {
    let p: HashSet<&Vec<Cell>> = pred.iter().collect();
    let g: HashSet<&Vec<Cell>> = gold.iter().collect();
    p == g
}

/// Dedupe keeping first-occurrence order (`list(dict.fromkeys(rows))`).
fn dedupe(rows: &[Vec<Cell>]) -> Vec<&Vec<Cell>> {
    let mut seen = HashSet::new();
    rows.iter().filter(|r| seen.insert(*r)).collect()
}

/// `calculate_row_match`: (match, pred-only, truth-only) fractions of the gold row width.
fn row_match(pred: &[Cell], gold: &[Cell]) -> (f64, f64, f64) {
    if gold.is_empty() {
        // Python would raise ZeroDivisionError (scored 0 by the harness).
        return (0.0, 0.0, 0.0);
    }
    let total = gold.len() as f64;
    let matches = pred.iter().filter(|v| gold.contains(v)).count() as f64;
    let pred_only = pred.len() as f64 - matches;
    let truth_only = gold.iter().filter(|v| !pred.contains(v)).count() as f64;
    (matches / total, pred_only / total, truth_only / total)
}

/// BIRD Soft-F1. Rows are compared positionally after dedupe; within a row,
/// column order does not matter.
pub(super) fn soft_f1(pred: &[Vec<Cell>], gold: &[Vec<Cell>]) -> f64 {
    if pred.is_empty() && gold.is_empty() {
        return 1.0;
    }
    let pred = dedupe(pred);
    let gold = dedupe(gold);
    let (mut tp, mut fp, mut fn_) = (0.0, 0.0, 0.0);
    for (i, gt_row) in gold.iter().enumerate() {
        match pred.get(i) {
            None => fn_ += 1.0,
            Some(p) => {
                let (m, po, to) = row_match(p, gt_row);
                tp += m;
                fp += po;
                fn_ += to;
            }
        }
    }
    fp += pred.len().saturating_sub(gold.len()) as f64;
    let precision = if tp + fp > 0.0 { tp / (tp + fp) } else { 0.0 };
    let recall = if tp + fn_ > 0.0 { tp / (tp + fn_) } else { 0.0 };
    if precision + recall > 0.0 {
        2.0 * precision * recall / (precision + recall)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str) -> Cell {
        Cell::Num(s.into())
    }
    fn t(s: &str) -> Cell {
        Cell::Text(s.into())
    }

    #[test]
    fn numeric_types_canonicalize_equal() {
        use agentic_core::result::TypedValue as V;
        assert_eq!(
            canonical(&V::Int64(12)),
            canonical(&V::Decimal("12.00".into()))
        );
        assert_eq!(canonical(&V::Int64(12)), canonical(&V::Float64(12.0)));
        assert_eq!(
            canonical(&V::Decimal("1.50".into())),
            canonical(&V::Float64(1.5))
        );
        assert_ne!(canonical(&V::Int64(12)), canonical(&V::Text("12".into())));
        assert_eq!(canonical(&V::Date(0)), t("1970-01-01"));
        assert_eq!(canonical(&V::Null), Cell::Null);
    }

    #[test]
    fn canonical_edge_cases() {
        use agentic_core::result::TypedValue as V;
        assert_eq!(canonical(&V::Float64(f64::NAN)), t("NaN"));
        assert_eq!(canonical(&V::Float64(f64::NEG_INFINITY)), t("-inf"));
        assert_eq!(canonical(&V::Float64(-0.0)), n("0"));
        assert_eq!(canonical(&V::Decimal("-0.50".into())), n("-0.5"));
        assert_eq!(canonical(&V::Decimal("100".into())), n("100"));
        assert_eq!(canonical(&V::Timestamp(0)), t("1970-01-01T00:00:00Z"));
        assert_eq!(
            canonical(&V::Timestamp(1_500_000)),
            t("1970-01-01T00:00:01.500Z")
        );
        assert_eq!(
            canonical(&V::Json(serde_json::json!({"a": 1}))),
            t("{\"a\":1}")
        );
        assert_eq!(canonical(&V::Bool(true)), Cell::Bool(true));
    }

    #[test]
    fn ex_ignores_row_order_and_duplicates() {
        let a = vec![
            vec![n("1"), t("x")],
            vec![n("2"), t("y")],
            vec![n("1"), t("x")],
        ];
        let b = vec![vec![n("2"), t("y")], vec![n("1"), t("x")]];
        assert!(execution_match(&a, &b));
    }

    #[test]
    fn ex_respects_column_order() {
        assert!(!execution_match(
            &[vec![n("1"), t("x")]],
            &[vec![t("x"), n("1")]]
        ));
    }

    #[test]
    fn ex_extra_column_fails() {
        assert!(!execution_match(&[vec![n("1"), t("x")]], &[vec![n("1")]]));
    }

    #[test]
    fn soft_f1_matches_bird_reference() {
        // Values computed with bird-bench/mini_dev evaluation_f1.calculate_f1_score.
        assert_eq!(soft_f1(&[], &[]), 1.0);
        assert_eq!(soft_f1(&[vec![n("1")]], &[vec![n("1")]]), 1.0);
        // pred has an extra column: tp=1, fp=1, fn=0 → p=.5 r=1 → f1=2/3
        assert!((soft_f1(&[vec![n("1"), t("x")]], &[vec![n("1")]]) - 2.0 / 3.0).abs() < 1e-9);
        // one gold row missing: tp=1, fp=0, fn=1 → f1=2/3
        assert!((soft_f1(&[vec![n("1")]], &[vec![n("1")], vec![n("2")]]) - 2.0 / 3.0).abs() < 1e-9);
        // column order does not matter for soft-F1
        assert_eq!(
            soft_f1(&[vec![t("x"), n("1")]], &[vec![n("1"), t("x")]]),
            1.0
        );
    }

    #[test]
    fn null_equals_null() {
        assert!(execution_match(&[vec![Cell::Null]], &[vec![Cell::Null]]));
    }
}
