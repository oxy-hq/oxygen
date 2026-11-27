//! SQL-literal formatting helpers for the simple-query INSERT path.
//!
//! DuckLake's pgwire surface doesn't support `$N` placeholders, so every
//! value is embedded directly in the SQL string. These helpers handle
//! the four shapes we need (string, NULL, timestamp, number) and the
//! one critical safety concern: escaping single quotes.

use chrono::{DateTime, Utc};

/// Single-quoted SQL string with `'` doubled. Use for any
/// untrusted-string value (track ids, model names, JSON blobs, etc.).
pub fn sql_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// `'value'` or `NULL`.
pub fn sql_opt_str(s: Option<&str>) -> String {
    match s {
        Some(v) => sql_str(v),
        None => "NULL".into(),
    }
}

/// ISO-8601 timestamp in single quotes.
pub fn sql_ts(t: DateTime<Utc>) -> String {
    sql_str(&t.to_rfc3339())
}

pub fn sql_opt_ts(t: Option<DateTime<Utc>>) -> String {
    match t {
        Some(v) => sql_ts(v),
        None => "NULL".into(),
    }
}

/// `f32` / `f64` literal. NaN / inf round-trip as NULL.
pub fn sql_opt_f32(v: Option<f32>) -> String {
    match v {
        Some(f) if f.is_finite() => format!("{f}"),
        _ => "NULL".into(),
    }
}

pub fn sql_opt_i32(v: Option<i32>) -> String {
    match v {
        Some(i) => format!("{i}"),
        None => "NULL".into(),
    }
}

pub fn sql_opt_i64(v: Option<i64>) -> String {
    match v {
        Some(i) => format!("{i}"),
        None => "NULL".into(),
    }
}

pub fn sql_i32(v: i32) -> String {
    format!("{v}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_are_doubled() {
        assert_eq!(sql_str("o'brien"), "'o''brien'");
    }

    #[test]
    fn null_and_value() {
        assert_eq!(sql_opt_str(None), "NULL");
        assert_eq!(sql_opt_str(Some("abc")), "'abc'");
    }

    #[test]
    fn nan_becomes_null() {
        assert_eq!(sql_opt_f32(Some(f32::NAN)), "NULL");
        assert_eq!(sql_opt_f32(Some(1.5)), "1.5");
    }
}
