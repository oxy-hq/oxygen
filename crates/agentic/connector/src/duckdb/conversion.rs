//! DuckDB `Value` → `CellValue` / `TypedValue` conversion helpers.

use duckdb::types::{TimeUnit, Value};

use agentic_core::result::{CellValue, TypedDataType, TypedValue};

/// Convert days since Unix epoch (1970-01-01) to an ISO date string (YYYY-MM-DD).
pub(super) fn epoch_days_to_iso(days: i32) -> String {
    // Algorithm: https://howardhinnant.github.io/date_algorithms.html (civil_from_days)
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe as i64 + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Ticks per second for a DuckDB timestamp unit, and the digits a fraction of
/// a second takes in it.
fn ticks_and_fraction_digits(unit: &TimeUnit) -> (i64, usize) {
    match unit {
        TimeUnit::Second => (1, 0),
        TimeUnit::Millisecond => (1_000, 3),
        TimeUnit::Microsecond => (1_000_000, 6),
        TimeUnit::Nanosecond => (1_000_000_000, 9),
    }
}

/// Convert a timestamp (in the given unit, since Unix epoch) to an ISO datetime string.
///
/// Floor division, not `/`: a pre-1970 instant is a negative tick count, and
/// truncating toward zero would read `20:17:40.5` as `20:17:41` with the
/// fraction taken from the wrong second. The fraction is zero-padded to the
/// unit's width: one microsecond is `.000001`, where a bare `{sub_secs}`
/// wrote `.1`.
pub(super) fn epoch_ts_to_iso(unit: &TimeUnit, value: i64) -> String {
    let (ticks, digits) = ticks_and_fraction_digits(unit);
    let secs = value.div_euclid(ticks);
    let sub_secs = value.rem_euclid(ticks);
    let days = secs.div_euclid(86_400) as i32;
    let time_secs = secs.rem_euclid(86_400);
    let h = time_secs / 3600;
    let m = (time_secs % 3600) / 60;
    let s = time_secs % 60;
    let date = epoch_days_to_iso(days);
    if sub_secs == 0 && h == 0 && m == 0 && s == 0 {
        date
    } else if sub_secs == 0 {
        format!("{date} {h:02}:{m:02}:{s:02}")
    } else {
        format!("{date} {h:02}:{m:02}:{s:02}.{sub_secs:0digits$}")
    }
}

/// Map a `duckdb::types::Value` to the connector-neutral [`CellValue`].
pub(super) fn duckdb_to_cell(v: Value) -> CellValue {
    match v {
        Value::Null => CellValue::Null,
        Value::Boolean(b) => CellValue::Number(if b { 1.0 } else { 0.0 }),
        Value::TinyInt(n) => CellValue::Number(n as f64),
        Value::SmallInt(n) => CellValue::Number(n as f64),
        Value::Int(n) => CellValue::Number(n as f64),
        Value::BigInt(n) => CellValue::Number(n as f64),
        Value::HugeInt(n) => CellValue::Number(n as f64),
        Value::UTinyInt(n) => CellValue::Number(n as f64),
        Value::USmallInt(n) => CellValue::Number(n as f64),
        Value::UInt(n) => CellValue::Number(n as f64),
        Value::UBigInt(n) => CellValue::Number(n as f64),
        Value::Float(f) => CellValue::Number(f as f64),
        Value::Double(f) => CellValue::Number(f),
        Value::Text(s) => CellValue::Text(s),
        Value::Enum(s) => CellValue::Text(s),
        Value::Blob(b) => CellValue::Text(format!("<blob {} bytes>", b.len())),
        Value::Date32(days) => CellValue::Text(epoch_days_to_iso(days)),
        Value::Timestamp(unit, value) => CellValue::Text(epoch_ts_to_iso(&unit, value)),
        Value::Time64(unit, value) => {
            let secs = match unit {
                TimeUnit::Second => value,
                TimeUnit::Millisecond => value / 1_000,
                TimeUnit::Microsecond => value / 1_000_000,
                TimeUnit::Nanosecond => value / 1_000_000_000,
            };
            let h = secs / 3600;
            let m = (secs % 3600) / 60;
            let s = secs % 60;
            CellValue::Text(format!("{h:02}:{m:02}:{s:02}"))
        }
        // Complex types — stringify so the LLM can read them.
        other => CellValue::Text(format!("{other:?}")),
    }
}

// ── Connector ─────────────────────────────────────────────────────────────────

/// DuckDB-backed connector for Parquet/CSV and in-process analytics.
///
/// `duckdb::Connection` uses `RefCell` internally and is not `Sync`.
/// Wrapping it in a `Mutex` gives the `Sync` needed by

/// Parse a DESCRIBE type string (e.g. `"INTEGER"`, `"DECIMAL(10,2)"`,
/// `"TIMESTAMP_NS"`) into a [`TypedDataType`].
///
/// Unrecognized or parameterised complex types (`LIST`, `STRUCT`, `MAP`,
/// `UNION`) fall through to [`TypedDataType::Unknown`]; callers should then
/// stringify row values for those columns.
pub(super) fn describe_type_to_typed(type_str: &str) -> TypedDataType {
    let up = type_str.to_ascii_uppercase();
    let trimmed = up.trim();

    // DECIMAL(p,s) — parse precision / scale.
    if let Some(rest) = trimmed
        .strip_prefix("DECIMAL(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let mut parts = rest.split(',').map(str::trim);
        let precision = parts
            .next()
            .and_then(|s| s.parse::<u8>().ok())
            .unwrap_or(38);
        let scale = parts.next().and_then(|s| s.parse::<i8>().ok()).unwrap_or(0);
        return TypedDataType::Decimal { precision, scale };
    }

    match trimmed {
        "BOOLEAN" | "BOOL" => TypedDataType::Bool,
        "TINYINT" | "INT1" | "SMALLINT" | "INT2" | "INTEGER" | "INT" | "INT4" | "UTINYINT"
        | "USMALLINT" => TypedDataType::Int32,
        "BIGINT" | "INT8" | "UINTEGER" | "UBIGINT" => TypedDataType::Int64,
        "HUGEINT" | "UHUGEINT" => TypedDataType::Decimal {
            precision: 38,
            scale: 0,
        },
        "FLOAT" | "REAL" | "FLOAT4" | "DOUBLE" | "FLOAT8" => TypedDataType::Float64,
        "VARCHAR" | "CHAR" | "BPCHAR" | "TEXT" | "STRING" | "UUID" => TypedDataType::Text,
        "BLOB" | "BYTEA" | "BINARY" | "VARBINARY" => TypedDataType::Bytes,
        "DATE" => TypedDataType::Date,
        "TIMESTAMP"
        | "TIMESTAMP_S"
        | "TIMESTAMP_MS"
        | "TIMESTAMP_US"
        | "TIMESTAMP_NS"
        | "TIMESTAMPTZ"
        | "TIMESTAMP WITH TIME ZONE"
        | "DATETIME" => TypedDataType::Timestamp,
        "JSON" => TypedDataType::Json,
        _ => TypedDataType::Unknown,
    }
}

/// Convert a `duckdb::types::Value` to a [`TypedValue`], preserving native
/// types wherever [`TypedValue`] has a representation for them.
///
/// The `data_type` hint is used to steer DECIMAL-like HugeInt / UBigInt values
/// and to format Date / Timestamp rendering, but the actual variant chosen is
/// driven by the value itself — callers don't need a perfectly-matching spec.
pub(super) fn duckdb_value_to_typed(v: Value, data_type: &TypedDataType) -> TypedValue {
    match v {
        Value::Null => TypedValue::Null,
        Value::Boolean(b) => TypedValue::Bool(b),
        Value::TinyInt(n) => TypedValue::Int32(n as i32),
        Value::SmallInt(n) => TypedValue::Int32(n as i32),
        Value::Int(n) => TypedValue::Int32(n),
        Value::BigInt(n) => TypedValue::Int64(n),
        Value::UTinyInt(n) => TypedValue::Int32(n as i32),
        Value::USmallInt(n) => TypedValue::Int32(n as i32),
        Value::UInt(n) => TypedValue::Int64(n as i64),
        // u64 → i64 lossy; route through Decimal to preserve the full range.
        Value::UBigInt(n) => match i64::try_from(n) {
            Ok(v) => TypedValue::Int64(v),
            Err(_) => TypedValue::Decimal(n.to_string()),
        },
        // i128 — no direct TypedValue; serialize as Decimal string.
        Value::HugeInt(n) => TypedValue::Decimal(n.to_string()),
        Value::Float(f) => TypedValue::Float64(f as f64),
        Value::Double(f) => TypedValue::Float64(f),
        Value::Decimal(d) => TypedValue::Decimal(d.to_string()),
        Value::Text(s) => TypedValue::Text(s),
        Value::Enum(s) => TypedValue::Text(s),
        Value::Blob(b) => TypedValue::Bytes(b),
        Value::Date32(days) => TypedValue::Date(days),
        Value::Timestamp(unit, value) => {
            let micros = match unit {
                TimeUnit::Second => value.saturating_mul(1_000_000),
                TimeUnit::Millisecond => value.saturating_mul(1_000),
                TimeUnit::Microsecond => value,
                // Floor, not truncate: before 1970 the count is negative, and
                // `/` would carry `.500000001` to `.500001`.
                TimeUnit::Nanosecond => value.div_euclid(1_000),
            };
            TypedValue::Timestamp(micros)
        }
        // TIME-of-day, INTERVAL, and composite types (LIST, STRUCT, MAP, UNION)
        // have no direct TypedValue — emit the driver's string rendering.
        other => {
            let _ = data_type; // hint reserved for future shape-aware rendering
            TypedValue::Text(format!("{other:?}"))
        }
    }
}

pub(super) fn duckdb_to_cell_opt(v: Value) -> Option<CellValue> {
    match v {
        Value::Null => None,
        Value::Boolean(b) => Some(CellValue::Number(if b { 1.0 } else { 0.0 })),
        Value::TinyInt(n) => Some(CellValue::Number(n as f64)),
        Value::SmallInt(n) => Some(CellValue::Number(n as f64)),
        Value::Int(n) => Some(CellValue::Number(n as f64)),
        Value::BigInt(n) => Some(CellValue::Number(n as f64)),
        Value::HugeInt(n) => Some(CellValue::Number(n as f64)),
        Value::UTinyInt(n) => Some(CellValue::Number(n as f64)),
        Value::USmallInt(n) => Some(CellValue::Number(n as f64)),
        Value::UInt(n) => Some(CellValue::Number(n as f64)),
        Value::UBigInt(n) => Some(CellValue::Number(n as f64)),
        Value::Float(f) => Some(CellValue::Number(f as f64)),
        Value::Double(f) => Some(CellValue::Number(f)),
        Value::Text(s) => Some(CellValue::Text(s)),
        Value::Enum(s) => Some(CellValue::Text(s)),
        Value::Blob(_) => None,
        other => Some(CellValue::Text(format!("{other:?}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1969-07-20 20:17:40.500000001 UTC, in nanoseconds since the epoch.
    const MOON_LANDING_NS: i64 = -14_182_939_499_999_999;

    #[test]
    fn nanoseconds_before_1970_floor_to_the_microsecond() {
        let got = duckdb_value_to_typed(
            Value::Timestamp(TimeUnit::Nanosecond, MOON_LANDING_NS),
            &TypedDataType::Timestamp,
        );
        // 20:17:40.500000, not 20:17:40.500001.
        assert_eq!(got, TypedValue::Timestamp(-14_182_939_500_000));
    }

    #[test]
    fn nanoseconds_after_1970_drop_the_sub_microsecond_digits() {
        let got = duckdb_value_to_typed(
            Value::Timestamp(TimeUnit::Nanosecond, 1_710_074_096_789_123_456),
            &TypedDataType::Timestamp,
        );
        assert_eq!(got, TypedValue::Timestamp(1_710_074_096_789_123));
    }

    #[test]
    fn iso_rendering_before_1970_keeps_the_second_and_its_fraction() {
        assert_eq!(
            epoch_ts_to_iso(&TimeUnit::Nanosecond, MOON_LANDING_NS),
            "1969-07-20 20:17:40.500000001"
        );
        assert_eq!(
            epoch_ts_to_iso(&TimeUnit::Millisecond, -14_182_939_500),
            "1969-07-20 20:17:40.500"
        );
        assert_eq!(
            epoch_ts_to_iso(&TimeUnit::Second, -14_182_940),
            "1969-07-20 20:17:40"
        );
    }

    /// The fraction is as wide as its unit — 3, 6 or 9 digits — so one tick
    /// past the second is `.000001`, not `.1`, on either side of the epoch.
    #[test]
    fn iso_rendering_pads_a_short_fraction_to_the_unit_width() {
        // 2024-01-01 00:00:00 UTC is 1_704_067_200 seconds after the epoch.
        for (unit, after, before, fraction) in [
            (
                TimeUnit::Millisecond,
                1_704_067_200_005,
                -14_182_939_995,
                "005",
            ),
            (
                TimeUnit::Microsecond,
                1_704_067_200_000_001,
                -14_182_939_999_999,
                "000001",
            ),
            (
                TimeUnit::Nanosecond,
                1_704_067_200_000_000_001,
                -14_182_939_999_999_999,
                "000000001",
            ),
        ] {
            assert_eq!(
                epoch_ts_to_iso(&unit, after),
                format!("2024-01-01 00:00:00.{fraction}")
            );
            assert_eq!(
                epoch_ts_to_iso(&unit, before),
                format!("1969-07-20 20:17:40.{fraction}")
            );
        }
        // A fraction with a zero in the middle keeps it: 50 ms is `.050`.
        assert_eq!(
            epoch_ts_to_iso(&TimeUnit::Millisecond, 1_704_067_200_050),
            "2024-01-01 00:00:00.050"
        );
    }
}
