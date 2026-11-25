//! Unit tests of the literals, each rendered from a value's JSON lexeme.

use serde_json::value::RawValue;

use super::{before_christ, literal};
use crate::oltp_changes::relay::table::DuckType;

fn raw(json: &str) -> Box<RawValue> {
    RawValue::from_string(json.to_string()).expect("JSON")
}

#[test]
fn literals_follow_the_column_type() {
    use DuckType::*;
    let cases = [
        ("null", Integer, "CAST(NULL AS INTEGER)"),
        ("-42", Integer, "CAST(-42 AS INTEGER)"),
        ("1.5", Decimal(10, 2), "CAST(1.5 AS DECIMAL(10,2))"),
        ("1e-7", Double, "CAST(1e-7 AS DOUBLE)"),
        (r#""NaN""#, Double, "CAST('NaN' AS DOUBLE)"),
        (r#""-Infinity""#, Float, "CAST('-Infinity' AS FLOAT)"),
        ("true", Boolean, "true"),
        ("12", Varchar, "CAST('12' AS VARCHAR)"),
        (r#"["a", 1]"#, Varchar, r#"CAST('["a", 1]' AS VARCHAR)"#),
        (r#""a\"b""#, Varchar, r#"CAST('a"b' AS VARCHAR)"#),
        ("[]", List(Box::new(Integer)), "CAST([] AS INTEGER[])"),
    ];
    for (value, ty, sql) in cases {
        let got = literal(&raw(value), &ty);
        assert_eq!(got.as_deref(), Ok(sql), "{value} as {ty:?}");
    }
    for (value, ty) in [
        ("70000", SmallInt),
        ("1.5", BigInt),
        (r#""7""#, Integer),
        (r#""NaN""#, Decimal(10, 2)),
        (r#""not-a-uuid""#, Uuid),
        ("[1]", Integer),
        (r#"{"a": 1}"#, Date),
        ("true", Integer),
        ("1", Uuid),
    ] {
        assert!(literal(&raw(value), &ty).is_err(), "{value} as {ty:?}");
    }
}

/// A refusal is a hold's reason, stored and logged: it names the kind of
/// value and the type, never the value.
#[test]
fn a_refusal_never_quotes_the_value() {
    use DuckType::*;
    for (value, ty, secret, reason) in [
        (
            "70000",
            SmallInt,
            "70000",
            "a number out of range for SMALLINT",
        ),
        (
            r#""s3cret-not-a-uuid""#,
            Uuid,
            "s3cret",
            "a string that is not a UUID where UUID is expected",
        ),
        (
            r#""s3cret""#,
            Integer,
            "s3cret",
            "a string where INTEGER is expected",
        ),
        (
            r#"["s3cret"]"#,
            List(Box::new(Integer)),
            "s3cret",
            "a string where INTEGER is expected",
        ),
    ] {
        let refused = literal(&raw(value), &ty).unwrap_err();
        assert_eq!(refused, reason, "{value} as {ty:?}");
        assert!(!refused.contains(secret), "{refused}");
    }
}

#[test]
fn a_json_value_keeps_its_json_text() {
    let json = DuckType::of("jsonb");
    for (value, sql) in [
        (r#""abc""#, r#"CAST('"abc"' AS VARCHAR)"#),
        ("12.50", "CAST('12.50' AS VARCHAR)"),
        (
            r#"{"k": [1, "it's"]}"#,
            r#"CAST('{"k": [1, "it''s"]}' AS VARCHAR)"#,
        ),
        (
            r#"["a", {"b": null}]"#,
            r#"CAST('["a", {"b": null}]' AS VARCHAR)"#,
        ),
        ("null", "CAST(NULL AS VARCHAR)"),
    ] {
        assert_eq!(literal(&raw(value), &json).as_deref(), Ok(sql), "{value}");
    }
    // Text is not JSON: a string is its content, without quotes.
    let text = literal(&raw(r#""abc""#), &DuckType::Varchar);
    assert_eq!(text.as_deref(), Ok("CAST('abc' AS VARCHAR)"));
}

#[test]
fn a_bc_date_is_spelled_as_duckdb_reads_it() {
    assert_eq!(before_christ("0044-03-15 BC"), "0044-03-15 (BC)");
    let zoned = before_christ("0044-03-15T10:00:00+00:53:28 BC");
    assert_eq!(zoned, "0044-03-15 (BC) 10:00:00+00:53:28");
    assert_eq!(before_christ("2026-11-24T10:00:00"), "2026-11-24T10:00:00");
    let date = literal(&raw(r#""0044-03-15 BC""#), &DuckType::Date);
    assert_eq!(date.as_deref(), Ok("CAST('0044-03-15 (BC)' AS DATE)"));
}
