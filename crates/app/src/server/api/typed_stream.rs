//! Consumers for [`agentic_core::result::TypedRowStream`].
//!
//! Two sinks are provided — one per existing `SemanticQueryResponse` variant:
//!
//! - [`typed_stream_to_parquet`]: streams rows into an Arrow `RecordBatch`
//!   builder, flushes every 10K rows into a Parquet file in the workspace
//!   results directory, and returns the filename — matching the contract of
//!   `oxy_api_threads::result_files::store_result_file`.
//! - [`typed_stream_to_json_array`]: eagerly collects every row into the
//!   `Vec<Vec<String>>` shape the `ResultFormat::Json` branch expects.
//!
//! Both helpers preserve native column types (integers, dates, timestamps,
//! JSON) — critical for the Dev Portal SQL IDE, which reads the Parquet back
//! into DuckDB-WASM for client-side paging/sorting.

use std::sync::Arc;

use agentic_core::result::{ColumnSpec, TypedDataType, TypedRowStream, TypedValue};
use arrow::array::{
    ArrayRef, BinaryBuilder, BooleanBuilder, Date32Builder, Float64Builder, Int32Builder,
    Int64Builder, RecordBatch, StringBuilder, TimestampMicrosecondBuilder,
};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use futures::StreamExt;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::DiskSlot;
use oxy_shared::errors::OxyError;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use uuid::Uuid;

/// Maximum rows buffered per Arrow `RecordBatch` before flushing to Parquet.
/// Keeps memory bounded for large result sets while amortising writer overhead.
const BATCH_SIZE: usize = 10_000;

/// Convert a [`TypedRowStream`] into a Parquet file in the workspace results
/// directory. Returns `(file_name, row_count, connector_truncated)`:
/// - `file_name` — the generated filename (with `.parquet` extension);
/// - `row_count` — rows written (callers compare against their row cap);
/// - `connector_truncated` — `true` when the connector itself stopped early at
///   its [`ResultCap`](agentic_connector::ResultCap) byte/row backstop. This is
///   how byte-truncation of *wide* rows is surfaced: the row count can stay well
///   under the soft cap yet the result is still partial.
///
/// Column types map to Arrow as follows:
///
/// | [`TypedDataType`]      | Arrow `DataType`                              |
/// |------------------------|-----------------------------------------------|
/// | `Bool`                 | `Boolean`                                     |
/// | `Int32`                | `Int32`                                       |
/// | `Int64`                | `Int64`                                       |
/// | `Float64`              | `Float64`                                     |
/// | `Text`                 | `Utf8`                                        |
/// | `Bytes`                | `Binary`                                      |
/// | `Date`                 | `Date32`                                      |
/// | `Timestamp`            | `Timestamp(Microsecond, Some("UTC"))`         |
/// | `Decimal { .. }`       | `Utf8` (canonical decimal string — no loss)   |
/// | `Json`                 | `Utf8` (JSON re-serialized)                   |
/// | `Unknown`              | `Utf8`                                        |
/// Sentinel returned when a query has no column schema (DDL/DML or zero-column
/// SELECT). The caller should surface this as an empty result rather than
/// attempting to read a Parquet file — DuckDB WASM rejects schema-less files.
pub const EMPTY_RESULT_SENTINEL: &str = "__empty__";

pub async fn typed_stream_to_parquet<S: DiskSlot>(
    stream: TypedRowStream,
    workspace_manager: &WorkspaceManager<S>,
) -> Result<(String, usize, bool), OxyError> {
    let TypedRowStream {
        columns,
        mut rows,
        truncated,
    } = stream;

    // No column schema means DDL/DML or a connector that couldn't describe the
    // result set. Writing a zero-column Parquet file would break DuckDB WASM
    // ("Need at least one non-root column"). Signal an empty result instead.
    if columns.is_empty() {
        // Drain the row stream so the connector can clean up.
        while rows.next().await.is_some() {}
        return Ok((EMPTY_RESULT_SENTINEL.to_string(), 0, false));
    }

    let arrow_schema: Arc<Schema> = Arc::new(build_arrow_schema(&columns));

    // `results_dir`, not `get_results_dir`: a result parquet is a runtime
    // artifact mirrored to S3 and read back through that mirror, so it must be
    // writable on a replica too. The `WorkingCopy` accessor routes through the storage
    // layer and is unreachable without a working copy.
    let results_dir = workspace_manager.config_manager.results_dir();
    tokio::fs::create_dir_all(&results_dir)
        .await
        .map_err(|e| OxyError::IOError(format!("mkdir results dir: {e}")))?;

    let file_name = format!("{}.parquet", Uuid::new_v4());
    let dest_path = results_dir.join(&file_name);

    // Channel carries completed RecordBatches from the async row-streaming side
    // to the blocking Parquet writer. Unbounded so send() never blocks the
    // async executor.
    let (batch_tx, batch_rx) = std::sync::mpsc::channel::<RecordBatch>();

    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .build();
    let write_schema = arrow_schema.clone();
    let write_task = tokio::task::spawn_blocking(move || {
        let file = std::fs::File::create(&dest_path)
            .map_err(|e| OxyError::IOError(format!("create parquet file: {e}")))?;
        let mut writer = ArrowWriter::try_new(file, write_schema, Some(props))
            .map_err(|e| OxyError::RuntimeError(format!("parquet writer: {e}")))?;
        for batch in batch_rx {
            writer
                .write(&batch)
                .map_err(|e| OxyError::RuntimeError(format!("parquet write: {e}")))?;
        }
        writer
            .close()
            .map_err(|e| OxyError::RuntimeError(format!("parquet close: {e}")))
    });

    let col_count = columns.len();
    let mut buffer: Vec<Vec<TypedValue>> = Vec::with_capacity(BATCH_SIZE);
    let mut sent_any_batch = false;
    let mut total_rows: usize = 0;

    while let Some(row) = rows.next().await {
        let row = row.map_err(|e| OxyError::DBError(format!("row stream: {e}")))?;
        if row.len() != col_count {
            return Err(OxyError::RuntimeError(format!(
                "row width mismatch: expected {} columns, got {}",
                col_count,
                row.len()
            )));
        }
        total_rows += 1;
        buffer.push(row);
        if buffer.len() >= BATCH_SIZE {
            let batch = build_record_batch(&arrow_schema, &columns, &buffer)?;
            batch_tx
                .send(batch)
                .map_err(|_| OxyError::RuntimeError("parquet writer task terminated".into()))?;
            buffer.clear();
            sent_any_batch = true;
        }
    }
    if !buffer.is_empty() {
        let batch = build_record_batch(&arrow_schema, &columns, &buffer)?;
        batch_tx
            .send(batch)
            .map_err(|_| OxyError::RuntimeError("parquet writer task terminated".into()))?;
        sent_any_batch = true;
    }
    if !sent_any_batch {
        // Zero rows: write an empty RecordBatch so the Parquet file contains at
        // least one row group. DuckDB WASM's parquet_scan cannot infer the schema
        // from a file with no row groups and throws "Need at least one non-root
        // column in the file".
        let empty = RecordBatch::new_empty(arrow_schema.clone());
        batch_tx
            .send(empty)
            .map_err(|_| OxyError::RuntimeError("parquet writer task terminated".into()))?;
    }
    // Drop sender so the blocking writer sees EOF and calls close().
    drop(batch_tx);

    write_task
        .await
        .map_err(|e| OxyError::RuntimeError(format!("parquet writer join: {e}")))??;

    // Mirror to S3 so any serve replica can serve this via
    // GET /{ws}/results/files/{id} on the round-robin fleet (best-effort;
    // no-op without a bucket). See server::runtime_artifact.
    let mirror_path = results_dir.join(&file_name);
    if let Ok(bytes) = tokio::fs::read(&mirror_path).await {
        let key =
            crate::server::runtime_artifact::result_key(workspace_manager.workspace_id, &file_name);
        crate::server::runtime_artifact::mirror(&key, bytes, "application/vnd.apache.parquet")
            .await;
    }

    // Read the producer truncation flag now that the row stream is fully
    // drained: streaming connectors only set it once they hit the cap mid-drain.
    let connector_truncated = agentic_core::result::truncation_flag_set(&truncated);
    Ok((file_name, total_rows, connector_truncated))
}

/// Collect a [`TypedRowStream`] into the `Vec<Vec<String>>` shape used by
/// `SemanticQueryResponse::Json`. The first row is the header (column names);
/// subsequent rows are cell strings.
pub async fn typed_stream_to_json_array(
    stream: TypedRowStream,
) -> Result<Vec<Vec<String>>, OxyError> {
    let TypedRowStream {
        columns, mut rows, ..
    } = stream;
    let header: Vec<String> = columns.iter().map(|c| c.name.clone()).collect();
    let mut out: Vec<Vec<String>> = vec![header];

    while let Some(row) = rows.next().await {
        let row = row.map_err(|e| OxyError::DBError(format!("row stream: {e}")))?;
        out.push(row.into_iter().map(typed_value_to_string).collect());
    }
    Ok(out)
}

/// Collect a [`TypedRowStream`] into `Vec<JsonValue>` where each row is a
/// `{column_name: value}` object with values preserved as native JSON types
/// (numbers stay numbers, bools stay bools, NULL → JSON null). This is the
/// shape custom-app producers return — charting libraries inside bundles
/// expect numeric axes, so we don't stringify everything the way the legacy
/// `Vec<Vec<String>>` shape does.
///
/// Returns `(rows, connector_truncated)`. `connector_truncated` is `true` when
/// the connector stopped early at its [`ResultCap`](agentic_connector::ResultCap)
/// backstop — the external `/query` + `/semantic-query` proxies must OR this
/// into their own length check (`len() == MAX_ROWS` for `/query`, which wraps
/// at the cap; `len() > MAX_ROWS` for `/semantic-query`, which wraps at
/// `MAX_ROWS + 1`), or a wide-row byte-truncation that stopped below
/// `MAX_ROWS` reads as a complete result.

pub async fn typed_stream_to_json_objects(
    stream: TypedRowStream,
) -> Result<(Vec<serde_json::Value>, bool), OxyError> {
    let TypedRowStream {
        columns,
        mut rows,
        truncated,
    } = stream;
    let names: Vec<String> = columns.iter().map(|c| c.name.clone()).collect();
    let mut out: Vec<serde_json::Value> = Vec::new();

    while let Some(row) = rows.next().await {
        let row = row.map_err(|e| OxyError::DBError(format!("row stream: {e}")))?;
        let mut obj = serde_json::Map::with_capacity(names.len());
        for (i, cell) in row.into_iter().enumerate() {
            let name = names.get(i).cloned().unwrap_or_else(|| format!("col_{i}"));
            obj.insert(name, typed_value_to_json(cell));
        }
        out.push(serde_json::Value::Object(obj));
    }
    // Read the producer truncation flag now that the stream is drained, and
    // return it alongside the rows: the custom-app `/query` + `/semantic-query`
    // proxies infer truncation from the row count, which misses a wide-row
    // byte-truncation that stopped *below* MAX_ROWS. Callers OR this in.
    let connector_truncated = agentic_core::result::truncation_flag_set(&truncated);
    Ok((out, connector_truncated))
}

/// Sibling of [`typed_value_to_string`] that preserves native JSON types.
/// Decimals, dates, timestamps, and JSON blobs round-trip as strings (or
/// nested JSON values for the JSON case) so consumers can re-parse without
/// guessing the original cell type from a flat string.
fn typed_value_to_json(v: TypedValue) -> serde_json::Value {
    use serde_json::Value;
    match v {
        TypedValue::Null => Value::Null,
        TypedValue::Bool(b) => Value::Bool(b),
        TypedValue::Int32(n) => Value::from(n),
        TypedValue::Int64(n) => Value::from(n),
        TypedValue::Float64(f) => serde_json::Number::from_f64(f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        TypedValue::Text(s) => Value::String(s),
        TypedValue::Bytes(b) => Value::String(format!("<{} bytes>", b.len())),
        TypedValue::Date(d) => Value::String(format_date(d)),
        TypedValue::Timestamp(t) => Value::String(format_timestamp_micros(t)),
        TypedValue::Decimal(s) => Value::String(s),
        TypedValue::Json(j) => j,
    }
}

// ── Schema construction ─────────────────────────────────────────────────────

fn typed_to_arrow(dt: &TypedDataType) -> DataType {
    match dt {
        TypedDataType::Bool => DataType::Boolean,
        TypedDataType::Int32 => DataType::Int32,
        TypedDataType::Int64 => DataType::Int64,
        TypedDataType::Float64 => DataType::Float64,
        TypedDataType::Text => DataType::Utf8,
        TypedDataType::Bytes => DataType::Binary,
        TypedDataType::Date => DataType::Date32,
        TypedDataType::Timestamp => DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
        // Decimal / JSON / Unknown survive round-trip as text — lossless for
        // decimals (canonical string form) and JSON (re-serialised). Parquet
        // readers can re-parse on the consumer side if needed.
        TypedDataType::Decimal { .. } | TypedDataType::Json | TypedDataType::Unknown => {
            DataType::Utf8
        }
    }
}

fn build_arrow_schema(columns: &[ColumnSpec]) -> Schema {
    let fields: Vec<Field> = columns
        .iter()
        .map(|c| Field::new(&c.name, typed_to_arrow(&c.data_type), true))
        .collect();
    Schema::new(fields)
}

// ── Column builders ─────────────────────────────────────────────────────────

/// Build one Arrow `RecordBatch` from a buffered slice of typed rows.
fn build_record_batch(
    schema: &Arc<Schema>,
    columns: &[ColumnSpec],
    rows: &[Vec<TypedValue>],
) -> Result<RecordBatch, OxyError> {
    let mut arrays: Vec<ArrayRef> = Vec::with_capacity(columns.len());
    for (col_idx, col) in columns.iter().enumerate() {
        let array =
            build_column_array(&col.data_type, rows.iter().map(|r| &r[col_idx])).map_err(|e| {
                OxyError::RuntimeError(format!("building arrow column '{}': {e}", col.name))
            })?;
        arrays.push(array);
    }
    RecordBatch::try_new(schema.clone(), arrays)
        .map_err(|e| OxyError::RuntimeError(format!("arrow record batch: {e}")))
}

/// Build a single column's Arrow array by iterating the corresponding cell of
/// each buffered row. Type mismatches (connector returned a `TypedValue`
/// variant incompatible with the declared column type) are flagged with a
/// clear message.
fn build_column_array<'a>(
    dt: &TypedDataType,
    values: impl Iterator<Item = &'a TypedValue>,
) -> Result<ArrayRef, String> {
    let values: Vec<&TypedValue> = values.collect();
    let len = values.len();

    macro_rules! mismatch {
        ($actual:expr) => {
            Err(format!(
                "expected {:?}, got {:?}",
                dt,
                std::mem::discriminant($actual)
            ))
        };
    }

    match dt {
        TypedDataType::Bool => {
            let mut b = BooleanBuilder::with_capacity(len);
            for v in values {
                match v {
                    TypedValue::Null => b.append_null(),
                    TypedValue::Bool(x) => b.append_value(*x),
                    other => return mismatch!(other),
                }
            }
            Ok(Arc::new(b.finish()))
        }
        TypedDataType::Int32 => {
            let mut b = Int32Builder::with_capacity(len);
            for v in values {
                match v {
                    TypedValue::Null => b.append_null(),
                    TypedValue::Int32(x) => b.append_value(*x),
                    other => return mismatch!(other),
                }
            }
            Ok(Arc::new(b.finish()))
        }
        TypedDataType::Int64 => {
            let mut b = Int64Builder::with_capacity(len);
            for v in values {
                match v {
                    TypedValue::Null => b.append_null(),
                    TypedValue::Int64(x) => b.append_value(*x),
                    // Allow widening from Int32 defensively.
                    TypedValue::Int32(x) => b.append_value(*x as i64),
                    other => return mismatch!(other),
                }
            }
            Ok(Arc::new(b.finish()))
        }
        TypedDataType::Float64 => {
            let mut b = Float64Builder::with_capacity(len);
            for v in values {
                match v {
                    TypedValue::Null => b.append_null(),
                    TypedValue::Float64(x) => b.append_value(*x),
                    other => return mismatch!(other),
                }
            }
            Ok(Arc::new(b.finish()))
        }
        TypedDataType::Text => {
            let mut b = StringBuilder::with_capacity(len, len * 16);
            for v in values {
                match v {
                    TypedValue::Null => b.append_null(),
                    TypedValue::Text(s) => b.append_value(s),
                    other => return mismatch!(other),
                }
            }
            Ok(Arc::new(b.finish()))
        }
        TypedDataType::Bytes => {
            let mut b = BinaryBuilder::with_capacity(len, len * 32);
            for v in values {
                match v {
                    TypedValue::Null => b.append_null(),
                    TypedValue::Bytes(x) => b.append_value(x.as_slice()),
                    other => return mismatch!(other),
                }
            }
            Ok(Arc::new(b.finish()))
        }
        TypedDataType::Date => {
            let mut b = Date32Builder::with_capacity(len);
            for v in values {
                match v {
                    TypedValue::Null => b.append_null(),
                    TypedValue::Date(d) => b.append_value(*d),
                    other => return mismatch!(other),
                }
            }
            Ok(Arc::new(b.finish()))
        }
        TypedDataType::Timestamp => {
            let mut b = TimestampMicrosecondBuilder::with_capacity(len).with_timezone("UTC");
            for v in values {
                match v {
                    TypedValue::Null => b.append_null(),
                    TypedValue::Timestamp(t) => b.append_value(*t),
                    other => return mismatch!(other),
                }
            }
            Ok(Arc::new(b.finish()))
        }
        TypedDataType::Decimal { .. } => {
            // Stored as the canonical decimal string. Callers who need
            // `Decimal128Array` can parse on the consumer side.
            let mut b = StringBuilder::with_capacity(len, len * 16);
            for v in values {
                match v {
                    TypedValue::Null => b.append_null(),
                    TypedValue::Decimal(s) => b.append_value(s),
                    // Int/float fallbacks: tolerate a connector that routed a
                    // NUMERIC through a narrower variant.
                    TypedValue::Int32(x) => b.append_value(x.to_string()),
                    TypedValue::Int64(x) => b.append_value(x.to_string()),
                    TypedValue::Float64(x) => b.append_value(x.to_string()),
                    other => return mismatch!(other),
                }
            }
            Ok(Arc::new(b.finish()))
        }
        TypedDataType::Json => {
            let mut b = StringBuilder::with_capacity(len, len * 32);
            for v in values {
                match v {
                    TypedValue::Null => b.append_null(),
                    TypedValue::Json(j) => b.append_value(j.to_string()),
                    TypedValue::Text(s) => b.append_value(s),
                    other => return mismatch!(other),
                }
            }
            Ok(Arc::new(b.finish()))
        }
        TypedDataType::Unknown => {
            let mut b = StringBuilder::with_capacity(len, len * 16);
            for v in values {
                b.append_option(typed_value_as_option_string(v));
            }
            Ok(Arc::new(b.finish()))
        }
    }
}

// ── Value → String helpers (for JSON sink + Unknown fallback) ───────────────

fn typed_value_as_option_string(v: &TypedValue) -> Option<String> {
    match v {
        TypedValue::Null => None,
        other => Some(typed_value_to_string(other.clone())),
    }
}

/// Render a single [`TypedValue`] as the display string used by the JSON
/// response shape. NULL becomes the empty string so the existing JSON contract
/// (`Vec<Vec<String>>`) is preserved.
fn typed_value_to_string(v: TypedValue) -> String {
    match v {
        TypedValue::Null => String::new(),
        TypedValue::Bool(b) => b.to_string(),
        TypedValue::Int32(n) => n.to_string(),
        TypedValue::Int64(n) => n.to_string(),
        TypedValue::Float64(f) => f.to_string(),
        TypedValue::Text(s) => s,
        TypedValue::Bytes(b) => format!("<{} bytes>", b.len()),
        // Arrow `Date32`: days since 1970-01-01.
        TypedValue::Date(d) => format_date(d),
        // Arrow `Timestamp(Microsecond, UTC)`: micros since epoch.
        TypedValue::Timestamp(t) => format_timestamp_micros(t),
        TypedValue::Decimal(s) => s,
        TypedValue::Json(j) => j.to_string(),
    }
}

fn format_date(days_since_epoch: i32) -> String {
    // Reuse the same algorithm used by the Airhouse parser to avoid a chrono
    // dep in this module. Keeping this in-file rather than shared since the
    // two copies will likely diverge as we add more backend-specific
    // formatting over time.
    let z = days_since_epoch as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe as i64 + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn format_timestamp_micros(micros: i64) -> String {
    let secs = micros.div_euclid(1_000_000);
    let us = micros.rem_euclid(1_000_000);
    let days = secs.div_euclid(86_400) as i32;
    let sod = secs.rem_euclid(86_400);
    let h = sod / 3600;
    let m = (sod % 3600) / 60;
    let s = sod % 60;
    let date = format_date(days);
    if us == 0 {
        format!("{date} {h:02}:{m:02}:{s:02}")
    } else {
        format!("{date} {h:02}:{m:02}:{s:02}.{us:06}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_to_arrow_covers_all_variants() {
        assert!(matches!(
            typed_to_arrow(&TypedDataType::Bool),
            DataType::Boolean
        ));
        assert!(matches!(
            typed_to_arrow(&TypedDataType::Int32),
            DataType::Int32
        ));
        assert!(matches!(
            typed_to_arrow(&TypedDataType::Date),
            DataType::Date32
        ));
        assert!(matches!(
            typed_to_arrow(&TypedDataType::Timestamp),
            DataType::Timestamp(TimeUnit::Microsecond, _)
        ));
        assert!(matches!(
            typed_to_arrow(&TypedDataType::Decimal {
                precision: 10,
                scale: 2
            }),
            DataType::Utf8
        ));
    }

    #[test]
    fn format_date_round_trips_epoch() {
        assert_eq!(format_date(0), "1970-01-01");
        assert_eq!(format_date(20_565), "2026-04-22");
    }

    #[test]
    fn format_timestamp_renders_date_only_when_time_is_zero() {
        assert_eq!(format_timestamp_micros(0), "1970-01-01 00:00:00");
        assert_eq!(
            format_timestamp_micros(1_500_000),
            "1970-01-01 00:00:01.500000"
        );
    }

    #[test]
    fn typed_value_to_string_handles_nulls_and_scalars() {
        assert_eq!(typed_value_to_string(TypedValue::Null), "");
        assert_eq!(typed_value_to_string(TypedValue::Int64(-7)), "-7");
        assert_eq!(typed_value_to_string(TypedValue::Text("hi".into())), "hi");
    }

    #[tokio::test]
    async fn json_objects_surfaces_connector_truncation_flag() {
        use agentic_core::result::{TypedRowError, TypedRowStream};
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;

        let columns = vec![ColumnSpec {
            name: "n".into(),
            data_type: TypedDataType::Int64,
        }];
        let rows: Vec<Result<Vec<TypedValue>, TypedRowError>> =
            vec![Ok(vec![TypedValue::Int64(1)])];
        // Producer flagged the result partial *below* the row cap — the wide-row
        // byte-truncation case the proxies' `len() == MAX_ROWS` check misses.
        let flag = Arc::new(AtomicBool::new(true));
        let stream = TypedRowStream::from_rows(columns, rows).with_truncation(flag);

        let (objects, truncated) = typed_stream_to_json_objects(stream).await.unwrap();
        assert_eq!(objects.len(), 1);
        assert!(truncated, "must surface the connector truncation flag");
    }

    #[tokio::test]
    async fn json_objects_not_truncated_without_flag() {
        use agentic_core::result::{TypedRowError, TypedRowStream};

        let columns = vec![ColumnSpec {
            name: "n".into(),
            data_type: TypedDataType::Int64,
        }];
        let rows: Vec<Result<Vec<TypedValue>, TypedRowError>> =
            vec![Ok(vec![TypedValue::Int64(1)])];
        let stream = TypedRowStream::from_rows(columns, rows); // truncated: None

        let (_objects, truncated) = typed_stream_to_json_objects(stream).await.unwrap();
        assert!(!truncated, "no flag → not truncated");
    }
}
