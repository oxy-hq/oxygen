//! Row-oriented typed conversion helpers for the ClickHouse backend.
//!
//! ClickHouse's HTTP `FORMAT JSONCompact` response gives us both column
//! metadata (with CH's rich type strings) and each row as an array of JSON
//! cells. This module translates those into [`TypedDataType`] /
//! [`TypedValue`] for [`execute_query_full`]: [`parse_ch_raw_cell`] takes a
//! cell's JSON text, so an integer wider than `u64` keeps its digits, and
//! [`parse_ch_cell`] the parsed `Value` for everything else.
//!
//! The type parser understands the wrappers CH sends in column metadata
//! (`Nullable(...)`, `LowCardinality(...)`) plus the common scalar types.
//! Composites (`Array`, `Tuple`, `Map`, `Nested`, etc.) map to
//! [`TypedDataType::Json`] — ClickHouse encodes them as JSON arrays /
//! objects in JSONCompact, so the already-deserialized `Value` threads
//! through unchanged.

use agentic_core::result::{ColumnSpec, TypedDataType, TypedRowError, TypedValue};
use serde_json::Value;

// ── Type mapping: ClickHouse type string → TypedDataType ────────────────────

/// Parse a ClickHouse column type string (as returned by the JSONCompact
/// `meta.type` field or by `system.columns.type`) into a [`TypedDataType`].
pub(crate) fn ch_type_to_typed(type_str: &str) -> TypedDataType {
    let inner = strip_type_wrappers(type_str.trim());

    // Prefix-matched types (parameterised).
    if let Some(rest) = inner.strip_prefix("Decimal") {
        return parse_decimal(rest);
    }
    if inner.starts_with("DateTime64") {
        return TypedDataType::Timestamp;
    }
    if inner.starts_with("DateTime") {
        return TypedDataType::Timestamp;
    }
    if inner.starts_with("FixedString") {
        return TypedDataType::Text;
    }
    if inner.starts_with("Enum") {
        return TypedDataType::Text;
    }
    // Composite types are stringified/JSONified downstream.
    if inner.starts_with("Array")
        || inner.starts_with("Tuple")
        || inner.starts_with("Map")
        || inner.starts_with("Nested")
        || inner.starts_with("AggregateFunction")
        || inner.starts_with("SimpleAggregateFunction")
    {
        return TypedDataType::Json;
    }

    match inner {
        "Bool" | "Boolean" => TypedDataType::Bool,
        "Int8" | "Int16" | "Int32" | "UInt8" | "UInt16" => TypedDataType::Int32,
        // `UInt64` keeps the `Int64` type so ordinary values stay JSON numbers;
        // one above `i64::MAX` decodes to a `Decimal` string holding its exact
        // digits (`parse_ch_cell`), the way DuckDB's `UBIGINT` does.
        "Int64" | "UInt32" | "UInt64" => TypedDataType::Int64,
        // Wider than any integer `TypedValue`: a `Decimal` string holding the
        // exact digits, read off the JSON text by `parse_ch_raw_cell`.
        "Int128" | "UInt128" | "Int256" | "UInt256" => TypedDataType::Decimal {
            precision: 38,
            scale: 0,
        },
        "Float32" | "Float64" => TypedDataType::Float64,
        "String" => TypedDataType::Text,
        "UUID" | "IPv4" | "IPv6" => TypedDataType::Text,
        "Date" | "Date32" => TypedDataType::Date,
        "JSON" | "Object('json')" | "Object(Nullable('json'))" => TypedDataType::Json,
        "Nothing" => TypedDataType::Unknown,
        _ => TypedDataType::Unknown,
    }
}

/// Peel off `Nullable(...)` and `LowCardinality(...)` wrappers. Both are
/// transparent for type mapping — the underlying CH value is still delivered
/// with the inner type's JSONCompact shape.
fn strip_type_wrappers(type_str: &str) -> &str {
    let mut s = type_str;
    loop {
        s = s.trim();
        if let Some(inner) = s
            .strip_prefix("Nullable(")
            .and_then(|v| v.strip_suffix(')'))
        {
            s = inner;
        } else if let Some(inner) = s
            .strip_prefix("LowCardinality(")
            .and_then(|v| v.strip_suffix(')'))
        {
            s = inner;
        } else {
            return s;
        }
    }
}

/// Parse `(p,s)` or `(p)` from `Decimal(18,2)` / `Decimal32(4)` / etc.
fn parse_decimal(rest: &str) -> TypedDataType {
    // Forms we accept:
    //   `Decimal(18, 2)` → caller passes `(18, 2)`
    //   `Decimal32(4)`   → caller passes `32(4)`
    //   `Decimal(38)`    → caller passes `(38)`
    let after_kind = rest.trim_start_matches(|c: char| c.is_ascii_digit());
    let inside = after_kind
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or("");
    let mut parts = inside.split(',').map(str::trim);
    let precision = parts
        .next()
        .and_then(|s| s.parse::<u8>().ok())
        .unwrap_or(38);
    let scale = parts.next().and_then(|s| s.parse::<i8>().ok()).unwrap_or(0);
    TypedDataType::Decimal { precision, scale }
}

// ── JSONCompact cell → TypedValue ────────────────────────────────────────────

/// Decode a JSONCompact cell from the JSON text ClickHouse wrote for it.
///
/// `serde_json` parses a bare integer wider than `u64` — an `Int128` through
/// `UInt256` past 2^64, or an integer-valued `Decimal` that wide — as an
/// `f64`, so [`parse_ch_cell`] would see `1.157920892373162e+77` and keep
/// that. Reading the digits off the text first keeps them exact, quoted by
/// the server or not. Everything else parses to a `Value` and takes the
/// [`parse_ch_cell`] path.
pub(crate) fn parse_ch_raw_cell(text: &str, col: &ColumnSpec) -> Result<TypedValue, TypedRowError> {
    match (&col.data_type, integer_literal(text)) {
        (TypedDataType::Decimal { .. }, Some(digits)) => {
            return Ok(TypedValue::Decimal(digits.to_string()));
        }
        // The commonest ClickHouse cell — ids, `count()`, every `UInt32` and
        // `Int64` column — so the digits parse in place, `i64` then `u64`, as
        // `parse_ch_cell` would. Only the refusal builds a `Value`, so that
        // `parse_ch_cell` words it.
        (TypedDataType::Int64, Some(digits)) => {
            if let Ok(n) = digits.parse::<i64>() {
                return Ok(TypedValue::Int64(n));
            }
            if let Ok(n) = digits.parse::<u64>() {
                return Ok(TypedValue::Decimal(n.to_string()));
            }
            return parse_ch_cell(&Value::String(digits.to_string()), col);
        }
        _ => {}
    }
    let value: Value = serde_json::from_str(text).map_err(|e| TypedRowError::TypeMappingError {
        column: col.name.clone(),
        native_type: format!("{:?}", col.data_type),
        message: format!("could not decode '{text}': {e}"),
    })?;
    parse_ch_cell(&value, col)
}

/// The digits of `text` when it is a JSON integer, bare (`-42`) or quoted
/// (`"42"`); `None` for anything else — a fraction, an exponent, `null`.
fn integer_literal(text: &str) -> Option<&str> {
    let text = text.trim();
    let digits = text
        .strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .unwrap_or(text);
    let unsigned = digits.strip_prefix('-').unwrap_or(digits);
    (!unsigned.is_empty() && unsigned.bytes().all(|b| b.is_ascii_digit())).then_some(digits)
}

/// Decode a single JSONCompact cell value into a [`TypedValue`].
///
/// Whether 64-bit and wider integers arrive quoted is the server's
/// `output_format_json_quote_64bit_integers`, off by default, so they come as
/// bare JSON numbers on most servers and as strings on some; every numeric
/// path tolerates both `Value::Number` and `Value::String`. Date / DateTime
/// cells always arrive as strings.
pub(crate) fn parse_ch_cell(value: &Value, col: &ColumnSpec) -> Result<TypedValue, TypedRowError> {
    if value.is_null() {
        return Ok(TypedValue::Null);
    }

    fn mapping_err(
        col: &ColumnSpec,
        value: &Value,
        detail: impl std::fmt::Display,
    ) -> TypedRowError {
        TypedRowError::TypeMappingError {
            column: col.name.clone(),
            native_type: format!("{:?}", col.data_type),
            message: format!("could not decode '{value}': {detail}"),
        }
    }

    match &col.data_type {
        TypedDataType::Bool => match value {
            Value::Bool(b) => Ok(TypedValue::Bool(*b)),
            Value::Number(n) => Ok(TypedValue::Bool(n.as_i64().unwrap_or(0) != 0)),
            Value::String(s) => match s.as_str() {
                "true" | "1" => Ok(TypedValue::Bool(true)),
                "false" | "0" => Ok(TypedValue::Bool(false)),
                _ => Err(mapping_err(col, value, "unrecognised bool literal")),
            },
            _ => Err(mapping_err(col, value, "expected bool")),
        },
        TypedDataType::Int32 => number_as_i64(value)
            .and_then(|n| i32::try_from(n).ok())
            .map(TypedValue::Int32)
            .ok_or_else(|| mapping_err(col, value, "not a 32-bit integer")),
        TypedDataType::Int64 => number_as_i64(value)
            .map(TypedValue::Int64)
            // A `UInt64` above `i64::MAX` — a bare JSON number `serde_json`
            // holds as a `u64`, or its quoted form — keeps its exact digits as
            // a `Decimal` string rather than failing the read or rounding.
            .or_else(|| number_as_u64(value).map(|n| TypedValue::Decimal(n.to_string())))
            .ok_or_else(|| mapping_err(col, value, "not a 64-bit integer")),
        TypedDataType::Float64 => number_as_f64(value)
            .map(TypedValue::Float64)
            .ok_or_else(|| mapping_err(col, value, "not a number")),
        TypedDataType::Text => match value {
            Value::String(s) => Ok(TypedValue::Text(s.clone())),
            other => Ok(TypedValue::Text(other.to_string())),
        },
        TypedDataType::Bytes => match value {
            Value::String(s) => Ok(TypedValue::Bytes(s.as_bytes().to_vec())),
            other => Ok(TypedValue::Bytes(other.to_string().into_bytes())),
        },
        TypedDataType::Date => value
            .as_str()
            .and_then(parse_date)
            .map(TypedValue::Date)
            .ok_or_else(|| mapping_err(col, value, "expected YYYY-MM-DD")),
        TypedDataType::Timestamp => value
            .as_str()
            .and_then(parse_timestamp_micros)
            .map(TypedValue::Timestamp)
            .ok_or_else(|| mapping_err(col, value, "expected YYYY-MM-DD HH:MM:SS[.fff]")),
        TypedDataType::Decimal { .. } => {
            let s = match value {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                other => other.to_string(),
            };
            Ok(TypedValue::Decimal(s))
        }
        TypedDataType::Json => Ok(TypedValue::Json(value.clone())),
        TypedDataType::Unknown => match value {
            Value::String(s) => Ok(TypedValue::Text(s.clone())),
            other => Ok(TypedValue::Text(other.to_string())),
        },
    }
}

fn number_as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn number_as_u64(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn number_as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

// ── Date / timestamp parsing (dependency-free, mirrors airhouse_typed) ──────

fn parse_date(s: &str) -> Option<i32> {
    let (y, m, d) = split_ymd(s)?;
    Some(days_from_civil(y, m, d))
}

fn parse_timestamp_micros(s: &str) -> Option<i64> {
    let (date_part, time_part) = match s.split_once(' ') {
        Some((d, t)) => (d, Some(t)),
        None => match s.split_once('T') {
            Some((d, t)) => (d, Some(t)),
            None => (s, None),
        },
    };
    let (y, m, d) = split_ymd(date_part)?;
    let days = days_from_civil(y, m, d) as i64;
    let sod_micros = match time_part {
        None => 0i64,
        Some(t) => parse_time_micros(t)?,
    };
    Some(days * 86_400 * 1_000_000 + sod_micros)
}

fn parse_time_micros(s: &str) -> Option<i64> {
    let s = s
        .split('+')
        .next()
        .unwrap_or(s)
        .trim_end_matches('Z')
        .trim();

    let mut parts = s.splitn(3, ':');
    let h: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let sec_raw = parts.next()?;

    let (sec_i, frac_us) = match sec_raw.split_once('.') {
        Some((whole, frac)) => {
            let sec_i: i64 = whole.parse().ok()?;
            let frac_truncated: String = frac.chars().take(6).collect();
            let frac_padded = format!("{frac_truncated:0<6}");
            let frac_us: i64 = frac_padded.parse().ok()?;
            (sec_i, frac_us)
        }
        None => (sec_raw.parse().ok()?, 0i64),
    };

    Some(h * 3_600_000_000 + m * 60_000_000 + sec_i * 1_000_000 + frac_us)
}

fn split_ymd(s: &str) -> Option<(i64, u32, u32)> {
    let mut parts = s.splitn(3, '-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    Some((y, m, d))
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i32 {
    let (y, m) = if m <= 2 { (y - 1, m + 12) } else { (y, m) };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let doy = (153 * (m - 3) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe as i64 - 719_468) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(data_type: TypedDataType) -> ColumnSpec {
        ColumnSpec {
            name: "c".into(),
            data_type,
        }
    }

    #[test]
    fn type_mapping_strips_nullable_and_lowcardinality() {
        assert_eq!(ch_type_to_typed("Nullable(Int32)"), TypedDataType::Int32);
        assert_eq!(
            ch_type_to_typed("LowCardinality(String)"),
            TypedDataType::Text
        );
        assert_eq!(
            ch_type_to_typed("Nullable(LowCardinality(String))"),
            TypedDataType::Text
        );
    }

    #[test]
    fn type_mapping_scalars() {
        assert_eq!(ch_type_to_typed("Bool"), TypedDataType::Bool);
        assert_eq!(ch_type_to_typed("Int32"), TypedDataType::Int32);
        assert_eq!(ch_type_to_typed("Int64"), TypedDataType::Int64);
        assert_eq!(ch_type_to_typed("UInt64"), TypedDataType::Int64);
        assert_eq!(ch_type_to_typed("Float64"), TypedDataType::Float64);
        assert_eq!(ch_type_to_typed("String"), TypedDataType::Text);
        assert_eq!(ch_type_to_typed("UUID"), TypedDataType::Text);
        assert_eq!(ch_type_to_typed("Date"), TypedDataType::Date);
        assert_eq!(ch_type_to_typed("DateTime"), TypedDataType::Timestamp);
        assert_eq!(
            ch_type_to_typed("DateTime64(3, 'UTC')"),
            TypedDataType::Timestamp
        );
    }

    #[test]
    fn type_mapping_decimals() {
        assert_eq!(
            ch_type_to_typed("Decimal(18, 2)"),
            TypedDataType::Decimal {
                precision: 18,
                scale: 2
            }
        );
        assert_eq!(
            ch_type_to_typed("Decimal32(4)"),
            TypedDataType::Decimal {
                precision: 4,
                scale: 0
            }
        );
    }

    #[test]
    fn type_mapping_composites_as_json() {
        assert_eq!(ch_type_to_typed("Array(Int32)"), TypedDataType::Json);
        assert_eq!(
            ch_type_to_typed("Tuple(Int32, String)"),
            TypedDataType::Json
        );
        assert_eq!(ch_type_to_typed("Map(String, Int64)"), TypedDataType::Json);
    }

    #[test]
    fn parse_cell_handles_null_and_bool() {
        assert_eq!(
            parse_ch_cell(&Value::Null, &col(TypedDataType::Bool)).unwrap(),
            TypedValue::Null
        );
        assert_eq!(
            parse_ch_cell(&Value::Bool(true), &col(TypedDataType::Bool)).unwrap(),
            TypedValue::Bool(true)
        );
        assert_eq!(
            parse_ch_cell(&serde_json::json!(1), &col(TypedDataType::Bool)).unwrap(),
            TypedValue::Bool(true)
        );
    }

    #[test]
    fn parse_cell_decodes_ints_from_number_or_string() {
        // Int64 commonly arrives as a string in JSONCompact.
        assert_eq!(
            parse_ch_cell(&Value::String("12345".into()), &col(TypedDataType::Int64)).unwrap(),
            TypedValue::Int64(12345)
        );
        // Int32 arrives as a JSON number.
        assert_eq!(
            parse_ch_cell(&serde_json::json!(42), &col(TypedDataType::Int32)).unwrap(),
            TypedValue::Int32(42)
        );
    }

    /// `UInt64` max overflows `i64`. It arrives as a bare JSON number unless
    /// the server quotes 64-bit integers; both forms keep the exact digits.
    #[test]
    fn parse_cell_routes_uint64_overflow_to_decimal_string() {
        let int64 = col(TypedDataType::Int64);
        for v in [
            serde_json::json!(18_446_744_073_709_551_615u64),
            Value::String("18446744073709551615".into()),
        ] {
            assert_eq!(
                parse_ch_cell(&v, &int64).unwrap(),
                TypedValue::Decimal("18446744073709551615".into()),
                "{v}"
            );
        }
        // The first value past `i64::MAX` is where the string form begins…
        assert_eq!(
            parse_ch_cell(&serde_json::json!(9_223_372_036_854_775_808u64), &int64).unwrap(),
            TypedValue::Decimal("9223372036854775808".into())
        );
        // …and `i64::MAX` itself is still an integer.
        assert_eq!(
            parse_ch_cell(&serde_json::json!(9_223_372_036_854_775_807u64), &int64).unwrap(),
            TypedValue::Int64(i64::MAX)
        );
        // Not an integer at all is still refused, not smuggled through as text.
        assert!(parse_ch_cell(&Value::String("wide".into()), &int64).is_err());
    }

    const UINT256_MAX: &str =
        "115792089237316195423570985008687907853269984665640564039457584007913129639935";
    const INT128_MIN: &str = "-170141183460469231731687303715884105728";

    fn wide() -> ColumnSpec {
        col(ch_type_to_typed("UInt256"))
    }

    /// A 128- or 256-bit integer past `u64` is an `f64` once `serde_json` has
    /// parsed it; decoded from the JSON text it keeps every digit, whether the
    /// server wrote it bare or quoted.
    #[test]
    fn raw_cell_keeps_wide_integers_exact() {
        for text in [UINT256_MAX.to_string(), format!("\"{UINT256_MAX}\"")] {
            assert_eq!(
                parse_ch_raw_cell(&text, &wide()).unwrap(),
                TypedValue::Decimal(UINT256_MAX.into()),
                "{text}"
            );
        }
        assert_eq!(
            parse_ch_raw_cell(INT128_MIN, &col(ch_type_to_typed("Int128"))).unwrap(),
            TypedValue::Decimal(INT128_MIN.into())
        );
        // What the parsed path makes of the same text — the lossy shape the
        // raw path exists to avoid.
        assert_eq!(
            parse_ch_cell(&serde_json::from_str(UINT256_MAX).unwrap(), &wide()).unwrap(),
            TypedValue::Decimal("1.157920892373162e+77".into())
        );
    }

    #[test]
    fn raw_cell_takes_the_int64_route_for_64_bit_columns() {
        let int64 = col(TypedDataType::Int64);
        assert_eq!(
            parse_ch_raw_cell("42", &int64).unwrap(),
            TypedValue::Int64(42)
        );
        assert_eq!(
            parse_ch_raw_cell("\"-42\"", &int64).unwrap(),
            TypedValue::Int64(-42)
        );
        assert_eq!(
            parse_ch_raw_cell("18446744073709551615", &int64).unwrap(),
            TypedValue::Decimal("18446744073709551615".into())
        );
        // The boundary, and the quoted form of a value past it.
        assert_eq!(
            parse_ch_raw_cell("9223372036854775807", &int64).unwrap(),
            TypedValue::Int64(i64::MAX)
        );
        assert_eq!(
            parse_ch_raw_cell("-9223372036854775808", &int64).unwrap(),
            TypedValue::Int64(i64::MIN)
        );
        assert_eq!(
            parse_ch_raw_cell("\"9223372036854775808\"", &int64).unwrap(),
            TypedValue::Decimal("9223372036854775808".into())
        );
    }

    /// An integer that fits neither `i64` nor `u64` is refused with the
    /// column, its type and the digits it could not hold. The wording is
    /// `parse_ch_cell`'s, whichever way the digits arrived.
    #[test]
    fn raw_cell_refuses_an_integer_past_u64_in_a_64_bit_column() {
        let int64 = col(TypedDataType::Int64);
        for text in [
            "123456789012345678901234567890",
            "\"123456789012345678901234567890\"",
        ] {
            match parse_ch_raw_cell(text, &int64).unwrap_err() {
                TypedRowError::TypeMappingError {
                    column,
                    native_type,
                    message,
                } => {
                    assert_eq!(column, "c");
                    assert_eq!(native_type, "Int64");
                    assert_eq!(
                        message,
                        "could not decode '\"123456789012345678901234567890\"': \
                         not a 64-bit integer"
                    );
                }
                other => panic!("expected a type-mapping error, got {other:?}"),
            }
        }
    }

    /// Anything that is not an integer literal parses as before: a decimal
    /// fraction, `null`, a composite, a string.
    #[test]
    fn raw_cell_parses_everything_else_as_a_value() {
        let decimal = col(TypedDataType::Decimal {
            precision: 18,
            scale: 4,
        });
        assert_eq!(
            parse_ch_raw_cell("1234.5678", &decimal).unwrap(),
            TypedValue::Decimal("1234.5678".into())
        );
        assert_eq!(
            parse_ch_raw_cell("null", &wide()).unwrap(),
            TypedValue::Null
        );
        assert_eq!(
            parse_ch_raw_cell("[1, 2]", &col(TypedDataType::Json)).unwrap(),
            TypedValue::Json(serde_json::json!([1, 2]))
        );
        assert_eq!(
            parse_ch_raw_cell("\"12\"", &col(TypedDataType::Text)).unwrap(),
            TypedValue::Text("12".into())
        );
        assert!(parse_ch_raw_cell("not json", &wide()).is_err());
    }

    #[test]
    fn integer_literal_accepts_bare_and_quoted_integers_only() {
        assert_eq!(integer_literal("12"), Some("12"));
        assert_eq!(integer_literal("-12"), Some("-12"));
        assert_eq!(integer_literal(" \"12\" "), Some("12"));
        assert_eq!(integer_literal("0"), Some("0"));
        for not_an_integer in ["1.0", "1e5", "-", "", "\"\"", "\"abc\"", "null", "-1.5"] {
            assert_eq!(integer_literal(not_an_integer), None, "{not_an_integer}");
        }
    }

    #[test]
    fn parse_cell_date_and_timestamp() {
        assert_eq!(
            parse_ch_cell(
                &Value::String("1970-01-01".into()),
                &col(TypedDataType::Date)
            )
            .unwrap(),
            TypedValue::Date(0)
        );
        assert_eq!(
            parse_ch_cell(
                &Value::String("1970-01-01 00:00:00".into()),
                &col(TypedDataType::Timestamp)
            )
            .unwrap(),
            TypedValue::Timestamp(0)
        );
        assert_eq!(
            parse_ch_cell(
                &Value::String("1970-01-01 00:00:01.5".into()),
                &col(TypedDataType::Timestamp)
            )
            .unwrap(),
            TypedValue::Timestamp(1_500_000)
        );
    }

    #[test]
    fn parse_cell_decimal_preserves_string() {
        let v = Value::String("123.4500".into());
        assert_eq!(
            parse_ch_cell(
                &v,
                &col(TypedDataType::Decimal {
                    precision: 10,
                    scale: 4
                })
            )
            .unwrap(),
            TypedValue::Decimal("123.4500".into())
        );
    }

    #[test]
    fn parse_cell_json_passes_through_arrays() {
        let v = serde_json::json!([1, 2, 3]);
        match parse_ch_cell(&v, &col(TypedDataType::Json)).unwrap() {
            TypedValue::Json(j) => assert_eq!(j, v),
            other => panic!("expected Json, got {other:?}"),
        }
    }
}
