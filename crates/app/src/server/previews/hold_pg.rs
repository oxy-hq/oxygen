//! Postgres behind a [`super::HoldingConnector`]: a read-only session, and
//! reads that do not need a temp table.
//!
//! **The session.** The classifier cannot see inside a function, so `SELECT
//! smuggle()` is a read to it even when `smuggle` inserts. On Postgres the
//! holding connector therefore also makes the session itself read-only
//! ([`READ_ONLY_SESSION`], before every statement it forwards), and the
//! database refuses the write. A session `SET` binds the statement after it
//! because `PostgresConnector` memoizes one client for its lifetime and never
//! reconnects, and every holding connector is built around its own inner
//! connector (`project_ctx::build_connector_for`, `preview_ctx::held_connector`).
//! It is re-sent each time, not once, because a function can call
//! `set_config('default_transaction_read_only', 'off', false)` from its body,
//! unseen, and the statement after it would then write. The `SET` and the
//! statement take the shared client's lock separately, so the holding
//! connector holds a lock of its own from the `SET` until the forwarded call
//! returns: concurrent calls through one connector (a preview run shares one
//! per database, an agent fans out) run `SET`, statement, `SET`, statement,
//! never interleaved. That is the guarantee: every statement a holding
//! connector forwards starts in a read-only transaction. A per-statement
//! prefix would not do: the connector runs each statement of a script as its
//! own autocommit batch.
//!
//! **The sample.** `PostgresConnector::execute_query` samples through `CREATE
//! TEMP TABLE _agentic_tmp AS (…)`, which a read-only session refuses
//! ("cannot execute CREATE TABLE AS in a read-only transaction"). So a held
//! Postgres `execute_query` is served by [`sample`] from `execute_query_full`
//! (a prepared statement over an inline subquery, no temp table), shaped as
//! the connector's own sampler shapes it. What still differs is listed on
//! [`sample`].
//!
//! **Known limits.** Redshift is built on the same connector and reports the
//! Postgres dialect, so it is told apart by its configured database type
//! (`hold::Session::for_type`) and gets no `SET` and no rerouted sample: there
//! the classifier is the whole guard, as before, and `SELECT my_proc()` can
//! still write through a function. The guarantee also covers only statements
//! sent through the holding connector; anything holding the inner connector
//! directly is outside it (none does: each is built for its holder). `dblink` opens its own
//! connection, outside this session. Side effects a read-only transaction
//! allows (`pg_notify`, `pg_terminate_backend`, advisory locks) are not
//! writes this guards. A **transaction-mode pooler** in front of the database
//! (PgBouncer `pool_mode = transaction`, Supavisor's transaction port) may
//! hand the `SET` and the statement after it to different server backends —
//! one client connection is not one server session there — so the read-only
//! session is not guaranteed behind one, and the classifier is again the
//! whole guard. A session-mode pooler or a direct connection keeps it.

use std::collections::HashSet;

use agentic_connector::{
    ColumnStats, ConnectorError, DatabaseConnector, ExecutionResult, ResultSummary,
};
use agentic_core::result::{
    CellValue, QueryResult, QueryRow, TypedDataType, TypedValue, truncation_flag_set,
};
use futures::StreamExt;

/// Makes every later transaction on the session read-only.
pub const READ_ONLY_SESSION: &str = "SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY";

/// `execute_query`'s contract — a bounded sample, the row count, per-column
/// stats — served from `inner.execute_query_full`, in one pass.
///
/// Matches the connector's sampler: cells are each value's `::TEXT` form read
/// back the way it reads them (anything that parses as a number is a number,
/// so `'02134'` is `2134` on both paths); `null_count`, `distinct_count`, `min`
/// and `max` cover every column; `mean` and `std_dev` (population) only a
/// column whose every value is a number.
///
/// Still differs: `data_type` is the Postgres type name the column's typed
/// kind maps back to (`int2` reads `int4`, `varchar` reads `text`, `json`
/// reads `jsonb`, an interval or array `text`); a `timestamp` column renders
/// with `+00` and microseconds as chrono prints them; JSON renders without the
/// spaces Postgres puts in; text `min`/`max` compare bytes, not the column's
/// collation; and past the connector's result cap the count is of the rows
/// read, with `truncated` set. (A `bool` column samples here, where the
/// connector's `MIN(bool)` fails the whole query.)
pub async fn sample(
    inner: &dyn DatabaseConnector,
    sql: &str,
    sample_limit: u64,
) -> Result<ExecutionResult, ConnectorError> {
    let stream = inner.execute_query_full(sql).await?;
    let names: Vec<String> = stream.columns.iter().map(|c| c.name.clone()).collect();
    let mut stats: Vec<Acc> = stream
        .columns
        .iter()
        .map(|c| Acc::new(&c.name, typname(&c.data_type)))
        .collect();
    let mut rows = Vec::new();
    let mut total: u64 = 0;
    let mut body = stream.rows;
    while let Some(row) = body.next().await {
        let cells: Vec<CellValue> = row?.into_iter().map(|v| text_cell(text_of(v))).collect();
        for (acc, cell) in stats.iter_mut().zip(&cells) {
            acc.add(cell);
        }
        if total < sample_limit {
            rows.push(QueryRow(cells));
        }
        total += 1;
    }
    let truncated = (rows.len() as u64) < total || truncation_flag_set(&stream.truncated);
    Ok(ExecutionResult {
        result: QueryResult {
            columns: names,
            rows,
            total_row_count: total,
            truncated,
        },
        summary: ResultSummary {
            row_count: total,
            columns: stats.into_iter().map(Acc::finish).collect(),
        },
    })
}

/// The Postgres type name a typed column reads as (the connector's sampler
/// reports `pg_type.typname`).
fn typname(t: &TypedDataType) -> &'static str {
    match t {
        TypedDataType::Bool => "bool",
        TypedDataType::Int32 => "int4",
        TypedDataType::Int64 => "int8",
        TypedDataType::Float64 => "float8",
        TypedDataType::Decimal { .. } => "numeric",
        TypedDataType::Bytes => "bytea",
        TypedDataType::Date => "date",
        TypedDataType::Timestamp => "timestamptz",
        TypedDataType::Json => "jsonb",
        TypedDataType::Text | TypedDataType::Unknown => "text",
    }
}

/// A value's `::TEXT` form, as the connector's sampler casts every column.
fn text_of(v: TypedValue) -> Option<String> {
    Some(match v {
        TypedValue::Null => return None,
        TypedValue::Bool(b) => b.to_string(),
        TypedValue::Int32(n) => n.to_string(),
        TypedValue::Int64(n) => n.to_string(),
        TypedValue::Float64(n) => n.to_string(),
        TypedValue::Decimal(s) | TypedValue::Text(s) => s,
        TypedValue::Json(j) => j.to_string(),
        TypedValue::Bytes(b) => format!(
            "\\x{}",
            b.iter().map(|x| format!("{x:02x}")).collect::<String>()
        ),
        TypedValue::Date(days) => chrono::DateTime::from_timestamp(i64::from(days) * 86_400, 0)
            .map(|d| d.date_naive().to_string())
            .unwrap_or_else(|| days.to_string()),
        TypedValue::Timestamp(us) => chrono::DateTime::from_timestamp_micros(us)
            .map(|t| t.format("%Y-%m-%d %H:%M:%S%.f+00").to_string())
            .unwrap_or_else(|| us.to_string()),
    })
}

/// How the connector's sampler reads a `::TEXT` cell back
/// (`postgres::pg_text_to_cell`).
fn text_cell(text: Option<String>) -> CellValue {
    match text {
        None => CellValue::Null,
        Some(s) => {
            if let Ok(n) = s.parse::<i64>() {
                CellValue::Number(n as f64)
            } else if let Ok(n) = s.parse::<f64>() {
                CellValue::Number(n)
            } else {
                CellValue::Text(s)
            }
        }
    }
}

/// One column's running stats (Welford for the moments).
struct Acc {
    name: String,
    data_type: &'static str,
    nulls: u64,
    distinct: HashSet<String>,
    /// Every non-null value so far was a number.
    numeric: bool,
    n: u64,
    mean: f64,
    m2: f64,
    min: Option<CellValue>,
    max: Option<CellValue>,
}

impl Acc {
    fn new(name: &str, data_type: &'static str) -> Self {
        Self {
            name: name.to_string(),
            data_type,
            nulls: 0,
            distinct: HashSet::new(),
            numeric: true,
            n: 0,
            mean: 0.0,
            m2: 0.0,
            min: None,
            max: None,
        }
    }

    fn add(&mut self, cell: &CellValue) {
        match cell {
            CellValue::Null => {
                self.nulls += 1;
                return;
            }
            CellValue::Number(x) => {
                self.distinct.insert(x.to_string());
                self.n += 1;
                let delta = x - self.mean;
                self.mean += delta / self.n as f64;
                self.m2 += delta * (x - self.mean);
            }
            CellValue::Text(s) => {
                self.distinct.insert(s.clone());
                self.numeric = false;
            }
        }
        keep(&mut self.min, cell, std::cmp::Ordering::Less);
        keep(&mut self.max, cell, std::cmp::Ordering::Greater);
    }

    fn finish(self) -> ColumnStats {
        let (mean, std_dev) = if self.numeric && self.n > 0 {
            (Some(self.mean), Some((self.m2 / self.n as f64).sqrt()))
        } else {
            (None, None)
        };
        ColumnStats {
            name: self.name,
            data_type: Some(self.data_type.to_string()),
            null_count: self.nulls,
            distinct_count: Some(self.distinct.len() as u64),
            min: Some(self.min.unwrap_or(CellValue::Null)),
            max: Some(self.max.unwrap_or(CellValue::Null)),
            mean,
            std_dev,
        }
    }
}

/// Replace `slot` with `cell` when `cell` compares `want` against it. Numbers
/// order before text, whichever arrives first, so a column the sampler reads
/// as both has its smallest number as `min` and its largest text as `max`.
fn keep(slot: &mut Option<CellValue>, cell: &CellValue, want: std::cmp::Ordering) {
    let wins = match slot.as_ref() {
        None => true,
        Some(current) => order(cell, current) == Some(want),
    };
    if wins {
        *slot = Some(cell.clone());
    }
}

/// Numbers before text; like with like by value. `None` for a NaN or a null.
fn order(a: &CellValue, b: &CellValue) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering::{Greater, Less};
    match (a, b) {
        (CellValue::Number(x), CellValue::Number(y)) => x.partial_cmp(y),
        (CellValue::Text(x), CellValue::Text(y)) => Some(x.cmp(y)),
        (CellValue::Number(_), CellValue::Text(_)) => Some(Less),
        (CellValue::Text(_), CellValue::Number(_)) => Some(Greater),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::result::{ColumnSpec, TypedRowStream};

    #[test]
    fn cells_read_back_as_the_connectors_text_forms() {
        let cell = |v| text_cell(text_of(v));
        assert!(matches!(cell(TypedValue::Int64(3)), CellValue::Number(n) if n == 3.0));
        assert!(
            matches!(cell(TypedValue::Decimal("1.50".into())), CellValue::Number(n) if n == 1.5)
        );
        assert!(
            matches!(cell(TypedValue::Text("02134".into())), CellValue::Number(n) if n == 2134.0)
        );
        assert!(matches!(cell(TypedValue::Bool(true)), CellValue::Text(s) if s == "true"));
        assert!(matches!(cell(TypedValue::Date(0)), CellValue::Text(s) if s == "1970-01-01"));
        assert!(
            matches!(cell(TypedValue::Timestamp(0)), CellValue::Text(s) if s == "1970-01-01 00:00:00+00")
        );
        assert!(matches!(cell(TypedValue::Null), CellValue::Null));
    }

    #[test]
    fn stats_cover_nulls_distincts_numbers_and_text() {
        let mut num = Acc::new("n", "int8");
        for c in [
            CellValue::Number(1.0),
            CellValue::Null,
            CellValue::Number(3.0),
            CellValue::Number(3.0),
        ] {
            num.add(&c);
        }
        let s = num.finish();
        assert_eq!(s.null_count, 1);
        assert_eq!(s.distinct_count, Some(2));
        assert!(matches!(s.min, Some(CellValue::Number(x)) if x == 1.0));
        assert!(matches!(s.max, Some(CellValue::Number(x)) if x == 3.0));
        assert!((s.mean.unwrap() - 7.0 / 3.0).abs() < 1e-12);
        assert!((s.std_dev.unwrap() - (8.0f64 / 9.0).sqrt()).abs() < 1e-12);

        let mut text = Acc::new("t", "text");
        for c in ["b", "a", "c"] {
            text.add(&CellValue::Text(c.into()));
        }
        let s = text.finish();
        assert!(matches!(s.min, Some(CellValue::Text(ref x)) if x == "a"));
        assert!(matches!(s.max, Some(CellValue::Text(ref x)) if x == "c"));
        assert_eq!(s.mean, None, "a text column has no mean");

        for cells in [
            [
                CellValue::Number(2.0),
                CellValue::Text("x".into()),
                CellValue::Number(1.0),
            ],
            [
                CellValue::Text("x".into()),
                CellValue::Number(2.0),
                CellValue::Number(1.0),
            ],
        ] {
            let mut mixed = Acc::new("m", "text");
            for c in &cells {
                mixed.add(c);
            }
            let s = mixed.finish();
            assert_eq!(s.mean, None, "`::DOUBLE PRECISION` would fail");
            assert!(
                matches!(s.min, Some(CellValue::Number(x)) if x == 1.0),
                "{cells:?}"
            );
            assert!(
                matches!(s.max, Some(CellValue::Text(ref x)) if x == "x"),
                "{cells:?}"
            );
        }

        let s = Acc::new("e", "int4").finish();
        assert!(
            matches!(s.min, Some(CellValue::Null)),
            "as `MIN(x)::TEXT` of nothing"
        );
    }

    struct Full;

    #[async_trait::async_trait]
    impl DatabaseConnector for Full {
        fn dialect(&self) -> agentic_connector::SqlDialect {
            agentic_connector::SqlDialect::Postgres
        }
        async fn execute_query(&self, _: &str, _: u64) -> Result<ExecutionResult, ConnectorError> {
            panic!("the sampler must not use the temp-table path")
        }
        async fn execute_query_full(&self, _: &str) -> Result<TypedRowStream, ConnectorError> {
            Ok(TypedRowStream::from_rows(
                vec![ColumnSpec {
                    name: "id".into(),
                    data_type: TypedDataType::Int32,
                }],
                (1..=3).map(|i| Ok(vec![TypedValue::Int32(i)])).collect(),
            ))
        }
    }

    #[tokio::test]
    async fn a_sample_is_bounded_and_counts_every_row() {
        let r = sample(&Full, "SELECT id FROM t", 2).await.expect("sampled");
        assert_eq!(r.result.columns, vec!["id"]);
        assert_eq!(r.result.rows.len(), 2);
        assert_eq!(r.result.total_row_count, 3);
        assert!(r.result.truncated);
        assert_eq!(r.summary.row_count, 3);
        assert_eq!(r.summary.columns[0].data_type.as_deref(), Some("int4"));
    }
}
