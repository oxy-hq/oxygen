//! Row-oriented typed conversion helpers for the Postgres backend.
//!
//! [`PostgresConnector::execute_query_full`] uses the patterns here to:
//!
//! 1. Map a Postgres `typname` string (from `pg_attribute`) to a
//!    [`TypedDataType`] the caller can persist (e.g. Parquet).
//! 2. Build a per-column SELECT expression that casts problematic types
//!    (NUMERIC, UUID, intervals, arrays, …) into something
//!    `tokio_postgres`'s typed row accessors can decode without adding
//!    crate-specific feature dependencies.
//! 3. Decode a single row into [`TypedValue`]s given the pre-computed
//!    per-column decoding plan.
//!
//! Keeping this out of `postgres.rs` means the existing `execute_query`
//! path stays untouched.
//!
//! The Airhouse connector does NOT use these helpers — its extended-protocol
//! column metadata is incompatible with `tokio_postgres`'s row-index access
//! (see the module doc in `airhouse.rs`). Airhouse handles typed rows via
//! the simple-query protocol path in `airhouse_typed.rs`.

use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, Utc};
use tokio_postgres::Row;

use agentic_core::result::{ColumnSpec, TypedDataType, TypedRowError, TypedValue};

// ── Type mapping: Postgres typname → TypedDataType ───────────────────────────

/// Map a Postgres `pg_attribute.typname` string to a [`TypedDataType`].
///
/// Unrecognised types map to [`TypedDataType::Unknown`]; the companion
/// [`select_expr_for_pg_type`] will cast them to `TEXT` so the row decoder
/// always has a concrete Rust type to target.
pub(crate) fn pg_typname_to_typed(typname: &str) -> TypedDataType {
    match typname {
        "bool" => TypedDataType::Bool,
        "int2" => TypedDataType::Int32,
        "int4" => TypedDataType::Int32,
        "int8" => TypedDataType::Int64,
        "float4" | "float8" => TypedDataType::Float64,
        // NUMERIC / DECIMAL: precision/scale metadata is in `atttypmod` which
        // pg_attribute does not expose through typname. Use sentinel (38, 0).
        // The downstream caller (parquet writer) tolerates this — the value
        // itself is stored as the canonical decimal string.
        "numeric" => TypedDataType::Decimal {
            precision: 38,
            scale: 0,
        },
        "text" | "varchar" | "bpchar" | "char" | "name" => TypedDataType::Text,
        "bytea" => TypedDataType::Bytes,
        "date" => TypedDataType::Date,
        "timestamp" | "timestamptz" => TypedDataType::Timestamp,
        "json" | "jsonb" => TypedDataType::Json,
        "uuid" => TypedDataType::Text,
        _ => TypedDataType::Unknown,
    }
}

// ── SELECT expression strategy ───────────────────────────────────────────────

/// Build the `SELECT` fragment for a single column of the given Postgres type.
///
/// For types that `tokio_postgres`'s extended-protocol decoder handles natively
/// under our feature set, we return the bare quoted identifier. For everything
/// else we wrap in an explicit cast so the decoder always sees one of:
///
/// - `bool`, `int2`, `int4`, `int8`, `float4`, `float8` (native numeric)
/// - `text`, `varchar`, `bpchar` (native text)
/// - `date`, `timestamp`, `timestamptz` (via `with-chrono-0_4`)
/// - `json`, `jsonb` (via `with-serde_json-1`)
/// - `bytea`
///
/// The returned string is `"col"` or `"col"::CAST` — no alias.
pub(crate) fn select_expr_for_pg_type(quoted_col: &str, typname: &str) -> String {
    match typname {
        // Cast NUMERIC to DOUBLE PRECISION so we can decode via f64. We
        // re-format as a decimal string on the way out so the paired
        // `TypedDataType::Decimal` stays accurate for the column spec.
        "numeric" => format!("{quoted_col}::DOUBLE PRECISION"),

        // UUID has no feature-free decoder; cast to text.
        "uuid" => format!("{quoted_col}::TEXT"),

        // `"char"` is one byte, which `tokio_postgres` reads as an `i8`, not a
        // `String`; left bare, the decode failed with a message that named
        // neither the column nor the type. Cast it to text and it is the one
        // character it holds.
        "char" => format!("{quoted_col}::TEXT"),

        // Types we can decode natively — no cast needed.
        "bool" | "int2" | "int4" | "int8" | "float4" | "float8" | "text" | "varchar" | "bpchar"
        | "name" | "bytea" | "date" | "timestamp" | "timestamptz" | "json" | "jsonb" => {
            quoted_col.to_string()
        }

        // Anything else (intervals, arrays, ranges, user-defined, …) — cast
        // to text and let the row decoder emit a `TypedValue::Text`.
        _ => format!("{quoted_col}::TEXT"),
    }
}

// ── Row decoding ────────────────────────────────────────────────────────────

/// Unix epoch as a `NaiveDate`, used to compute `Date32` days-since-epoch.
fn epoch_date() -> NaiveDate {
    NaiveDate::from_ymd_opt(1970, 1, 1).expect("1970-01-01 is a valid date")
}

/// Decode a single row into [`TypedValue`]s following the column spec list.
///
/// Each column's `ColumnSpec::data_type` tells the decoder which
/// `row.try_get::<T>()` type parameter to use; NULLs become [`TypedValue::Null`].
/// Decoder errors bubble up as a single [`TypedRowError::TypeMappingError`]
/// for the first failing column.
pub(crate) fn decode_row(
    row: &Row,
    columns: &[ColumnSpec],
) -> Result<Vec<TypedValue>, TypedRowError> {
    let mut out = Vec::with_capacity(columns.len());
    for (idx, col) in columns.iter().enumerate() {
        let cell = decode_cell(row, idx, col)?;
        out.push(cell);
    }
    Ok(out)
}

fn decode_cell(row: &Row, idx: usize, col: &ColumnSpec) -> Result<TypedValue, TypedRowError> {
    fn mapping_err(col: &ColumnSpec, err: impl std::fmt::Display) -> TypedRowError {
        TypedRowError::TypeMappingError {
            column: col.name.clone(),
            native_type: format!("{:?}", col.data_type),
            message: err.to_string(),
        }
    }

    match &col.data_type {
        TypedDataType::Bool => match row.try_get::<_, Option<bool>>(idx) {
            Ok(Some(v)) => Ok(TypedValue::Bool(v)),
            Ok(None) => Ok(TypedValue::Null),
            Err(e) => Err(mapping_err(col, e)),
        },
        TypedDataType::Int32 => {
            // Try i32 first (int4/int2 → i32), then i16 (int2).
            match row.try_get::<_, Option<i32>>(idx) {
                Ok(Some(v)) => Ok(TypedValue::Int32(v)),
                Ok(None) => Ok(TypedValue::Null),
                Err(_) => match row.try_get::<_, Option<i16>>(idx) {
                    Ok(Some(v)) => Ok(TypedValue::Int32(v as i32)),
                    Ok(None) => Ok(TypedValue::Null),
                    Err(e) => Err(mapping_err(col, e)),
                },
            }
        }
        TypedDataType::Int64 => match row.try_get::<_, Option<i64>>(idx) {
            Ok(Some(v)) => Ok(TypedValue::Int64(v)),
            Ok(None) => Ok(TypedValue::Null),
            Err(e) => Err(mapping_err(col, e)),
        },
        TypedDataType::Float64 => match row.try_get::<_, Option<f64>>(idx) {
            Ok(Some(v)) => Ok(TypedValue::Float64(v)),
            Ok(None) => Ok(TypedValue::Null),
            Err(_) => match row.try_get::<_, Option<f32>>(idx) {
                Ok(Some(v)) => Ok(TypedValue::Float64(v as f64)),
                Ok(None) => Ok(TypedValue::Null),
                Err(e) => Err(mapping_err(col, e)),
            },
        },
        TypedDataType::Text => match row.try_get::<_, Option<String>>(idx) {
            Ok(Some(v)) => Ok(TypedValue::Text(v)),
            Ok(None) => Ok(TypedValue::Null),
            Err(e) => Err(mapping_err(col, e)),
        },
        TypedDataType::Bytes => match row.try_get::<_, Option<Vec<u8>>>(idx) {
            Ok(Some(v)) => Ok(TypedValue::Bytes(v)),
            Ok(None) => Ok(TypedValue::Null),
            Err(e) => Err(mapping_err(col, e)),
        },
        TypedDataType::Date => match row.try_get::<_, Option<NaiveDate>>(idx) {
            Ok(Some(d)) => {
                let days = d.num_days_from_ce() - epoch_date().num_days_from_ce();
                Ok(TypedValue::Date(days))
            }
            Ok(None) => Ok(TypedValue::Null),
            Err(e) => Err(mapping_err(col, e)),
        },
        TypedDataType::Timestamp => {
            // TIMESTAMPTZ → DateTime<Utc>; TIMESTAMP → NaiveDateTime.
            match row.try_get::<_, Option<DateTime<Utc>>>(idx) {
                Ok(Some(ts)) => {
                    let micros = ts.timestamp() * 1_000_000 + (ts.timestamp_subsec_micros() as i64);
                    Ok(TypedValue::Timestamp(micros))
                }
                Ok(None) => Ok(TypedValue::Null),
                Err(_) => match row.try_get::<_, Option<NaiveDateTime>>(idx) {
                    Ok(Some(ts)) => {
                        let micros = ts.and_utc().timestamp() * 1_000_000
                            + (ts.and_utc().timestamp_subsec_micros() as i64);
                        Ok(TypedValue::Timestamp(micros))
                    }
                    Ok(None) => Ok(TypedValue::Null),
                    Err(e) => Err(mapping_err(col, e)),
                },
            }
        }
        TypedDataType::Decimal { .. } => {
            // NUMERIC columns are cast to DOUBLE PRECISION in the SELECT, so
            // decode as f64 and re-format as the canonical decimal string.
            // Precision loss is acceptable at this layer — callers that need
            // exact decimals cast the column themselves.
            match row.try_get::<_, Option<f64>>(idx) {
                Ok(Some(v)) => Ok(TypedValue::Decimal(format_decimal(v))),
                Ok(None) => Ok(TypedValue::Null),
                Err(e) => Err(mapping_err(col, e)),
            }
        }
        TypedDataType::Json => match row.try_get::<_, Option<serde_json::Value>>(idx) {
            Ok(Some(v)) => Ok(TypedValue::Json(v)),
            Ok(None) => Ok(TypedValue::Null),
            Err(e) => Err(mapping_err(col, e)),
        },
        TypedDataType::Unknown => match row.try_get::<_, Option<String>>(idx) {
            Ok(Some(v)) => Ok(TypedValue::Text(v)),
            Ok(None) => Ok(TypedValue::Null),
            Err(e) => Err(mapping_err(col, e)),
        },
    }
}

/// Render an f64 as a decimal string. Strips trailing zeros after the decimal
/// point but keeps at least one fractional digit so consumers can tell
/// `TypedValue::Decimal("1.0")` from an integer literal.
fn format_decimal(v: f64) -> String {
    if v.fract() == 0.0 && v.is_finite() {
        format!("{v:.1}")
    } else {
        format!("{v}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typname_mapping_covers_common_types() {
        assert_eq!(pg_typname_to_typed("bool"), TypedDataType::Bool);
        assert_eq!(pg_typname_to_typed("int4"), TypedDataType::Int32);
        assert_eq!(pg_typname_to_typed("int8"), TypedDataType::Int64);
        assert_eq!(pg_typname_to_typed("float8"), TypedDataType::Float64);
        assert_eq!(pg_typname_to_typed("text"), TypedDataType::Text);
        assert_eq!(pg_typname_to_typed("date"), TypedDataType::Date);
        assert_eq!(pg_typname_to_typed("timestamptz"), TypedDataType::Timestamp);
        assert_eq!(pg_typname_to_typed("jsonb"), TypedDataType::Json);
        assert_eq!(pg_typname_to_typed("uuid"), TypedDataType::Text);
        assert_eq!(pg_typname_to_typed("interval"), TypedDataType::Unknown);
    }

    #[test]
    fn select_expr_casts_problematic_types() {
        assert_eq!(select_expr_for_pg_type("\"x\"", "int4"), "\"x\"");
        assert_eq!(
            select_expr_for_pg_type("\"x\"", "numeric"),
            "\"x\"::DOUBLE PRECISION"
        );
        assert_eq!(select_expr_for_pg_type("\"x\"", "uuid"), "\"x\"::TEXT");
        assert_eq!(select_expr_for_pg_type("\"x\"", "interval"), "\"x\"::TEXT");
        assert_eq!(select_expr_for_pg_type("\"x\"", "jsonb"), "\"x\"");
    }

    /// `"char"` (the one-byte type, distinct from `bpchar`) maps to text like
    /// its wider cousins, and its SELECT casts — the bare column decodes as an
    /// `i8` in `tokio_postgres`, which the text decoder refused.
    #[test]
    fn select_expr_casts_the_one_byte_char_to_text() {
        assert_eq!(pg_typname_to_typed("char"), TypedDataType::Text);
        assert_eq!(select_expr_for_pg_type("\"x\"", "char"), "\"x\"::TEXT");
        assert_eq!(select_expr_for_pg_type("\"x\"", "bpchar"), "\"x\"");
    }

    #[test]
    fn format_decimal_keeps_fraction_for_integer_valued_floats() {
        assert_eq!(format_decimal(1.0), "1.0");
        assert_eq!(format_decimal(-42.0), "-42.0");
    }

    #[test]
    fn format_decimal_renders_fractional_values() {
        // Not 3.14: `clippy::approx_constant` is deny-by-default and reads it
        // as a botched `PI`. The test wants any fractional value.
        assert_eq!(format_decimal(2.75), "2.75");
        assert_eq!(format_decimal(-0.5), "-0.5");
    }
}
