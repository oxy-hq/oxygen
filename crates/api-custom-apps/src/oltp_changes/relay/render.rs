//! The change tables as SQL, for a workspace's Airhouse (OLTP change stream
//! design §5.3, "Write", and §5.4).
//!
//! - **Statements are text.** Airhouse's Postgres wire takes no `$N`
//!   placeholders, so every value is a literal ([`literal`]), cast to its
//!   column's type. The writer and table names have already passed
//!   `oxy_oltp::schema::validate_name`; a column's name is its Postgres
//!   attribute's, any text. Every identifier is double-quoted, its `"`
//!   doubled. String literals double their `'` and, in DuckDB, have no
//!   backslash escapes.
//! - **Column names compare without case,** as DuckDB's identifiers do: a
//!   change table listing `ID` has the plan's `id`.
//! - **No keys and no indexes:** DuckLake has none.
//! - **Idempotent.** A batch's `INSERT` adds only the `(generation, seq)`s not
//!   already there, so a relay that died after the Airhouse commit and before
//!   it confirmed ships the batch again and adds nothing.
//! - **Bounded.** A table's rows go out as one `INSERT` per [`INSERT_ROWS`]
//!   ([`insert_statements`]), so a large source transaction never renders
//!   one huge statement.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, SecondsFormat, Utc};
use oxy_cameras::airhouse::escape::sql_str;
use oxy_oltp::changes::RESERVED_COLUMNS;
use serde_json::value::RawValue;
use uuid::Uuid;

use self::literal::cast;
pub use self::literal::literal;
use super::INSERT_ROWS;
use super::table::{ChangeTable, DuckType};

mod baselines;
mod literal;

pub use self::baselines::{
    Marker, MarkerKind, baselines_insert_sql, latest_complete_sql, view_columns, view_ddl,
};

/// The schema's progress table: the last batch shipped, one row.
pub const PROGRESS_TABLE: &str = "_progress";

/// The schema's baseline markers, one row per `b` or `c` log row.
pub const BASELINES_TABLE: &str = "_baselines";

/// The types of [`RESERVED_COLUMNS`], in their order: `_generation`, `_seq`,
/// `_txid`, `_op`, `_changed_at`, `_actor`, `_invocation_id`.
static RESERVED_TYPES: [DuckType; 7] = [
    DuckType::BigInt,
    DuckType::BigInt,
    DuckType::BigInt,
    DuckType::Varchar,
    DuckType::TimestampTz,
    DuckType::Varchar,
    DuckType::Uuid,
];

/// One captured change, ready to ship.
#[derive(Debug, Clone)]
pub struct ShipRow {
    pub generation: i64,
    pub seq: i64,
    pub txid: u64,
    /// The `_op` written: `insert`, `update`, `delete`, `truncate` or `snapshot`.
    pub op: &'static str,
    pub changed_at: DateTime<Utc>,
    /// The verified actor, or `unverified`.
    pub actor: String,
    pub invocation: Option<Uuid>,
    pub schema_version: i32,
    /// Column name to value, for the columns `schema_version` captured, each
    /// value its JSON lexeme ([`parse_image`]). None for a truncate. A
    /// change-table column the image lacks is written null.
    pub image: Option<BTreeMap<String, Box<RawValue>>>,
}

/// A log row's image (`row_image::text`) as [`ShipRow::image`]: each column's
/// value kept as its JSON lexeme, never parsed into an `f64`, so a `numeric`
/// keeps every digit. `Err` says where the image stopped parsing, never what
/// it held: a `serde_json` error can quote the value it met.
pub fn parse_image(text: &str) -> Result<BTreeMap<String, Box<RawValue>>, String> {
    serde_json::from_str(text).map_err(|e| {
        format!(
            "row_image is not an object of columns ({:?} error at line {}, column {})",
            e.classify(),
            e.line(),
            e.column()
        )
    })
}

/// A value of a batch's row that cannot be written to its column. Its table
/// is held, and its rows stay in the log. It names the row, the column and
/// the kind of mismatch, never the value: its text is a hold's reason, stored
/// and logged (design §7).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("seq {seq}, column `{column}`: {reason}")]
pub struct Unrenderable {
    pub seq: i64,
    pub column: String,
    pub reason: String,
}

/// `writer`'s schema in Airhouse: `oltp_<writer>`, never the app's own
/// `app_<writer>`, which the app's credential can write.
pub fn schema_name(writer: &str) -> String {
    format!("oltp_{writer}")
}

fn quote(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// `"oltp_<writer>"."<table>"`.
fn qualified(writer: &str, table: &str) -> String {
    format!("{}.{}", quote(&schema_name(writer)), quote(table))
}

/// The change table of declared table `table`, unqualified: `<table>__changes`,
/// as [`columns_sql`] names it.
pub fn changes_table_name(table: &str) -> String {
    format!("{table}__changes")
}

/// `"oltp_<writer>"."<table>__changes"`.
fn changes_table(writer: &str, table: &ChangeTable) -> String {
    qualified(writer, &changes_table_name(&table.table))
}

/// The schema, its `_progress` and its `_baselines` table, each made only when
/// missing.
pub fn ensure_schema_ddl(writer: &str) -> Vec<String> {
    vec![
        format!(
            "CREATE SCHEMA IF NOT EXISTS {}",
            quote(&schema_name(writer))
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS {} (generation BIGINT NOT NULL, batch BIGINT NOT NULL, \
             shipped_at TIMESTAMPTZ NOT NULL)",
            qualified(writer, PROGRESS_TABLE)
        ),
        baselines::baselines_ddl(writer),
    ]
}

/// What brings `table`'s change table up to its plan. `existing` is the
/// change table's columns as [`columns_sql`] read them, `None` when it has
/// none: then one `CREATE TABLE` with the attribute columns, then the `_`
/// ones. Otherwise one `ALTER TABLE … ADD COLUMN` per attribute column it
/// lacks, by name without case; a column is never dropped, renamed or
/// retyped.
///
/// Both are `IF NOT EXISTS`, since Airhouse may answer `information_schema`
/// from a cache a minute old.
pub fn table_ddl(writer: &str, table: &ChangeTable, existing: Option<&[String]>) -> Vec<String> {
    let target = changes_table(writer, table);
    let Some(existing) = existing else {
        let attributes = table.columns.iter().map(|c| (c.name.as_str(), &c.ty));
        let reserved = RESERVED_COLUMNS.iter().copied().zip(RESERVED_TYPES.iter());
        let columns: Vec<String> = attributes
            .chain(reserved)
            .map(|(name, ty)| format!("{} {}", quote(name), ty.sql()))
            .collect();
        return vec![format!(
            "CREATE TABLE IF NOT EXISTS {target} ({})",
            columns.join(", ")
        )];
    };
    table
        .columns
        .iter()
        .filter(|c| !existing.iter().any(|e| e.eq_ignore_ascii_case(&c.name)))
        .map(|c| {
            format!(
                "ALTER TABLE {target} ADD COLUMN IF NOT EXISTS {} {}",
                quote(&c.name),
                c.ty.sql()
            )
        })
        .collect()
}

/// Every column of every table in `writer`'s schema, as `table_name,
/// column_name`, in column order.
pub fn columns_sql(writer: &str) -> String {
    format!(
        "SELECT table_name, column_name FROM information_schema.columns \
         WHERE table_schema = {} ORDER BY table_name, ordinal_position",
        sql_str(&schema_name(writer))
    )
}

/// The last batch shipped, as `generation, batch`; no row before the first.
pub fn progress_read_sql(writer: &str) -> String {
    format!(
        "SELECT generation, batch FROM {} ORDER BY generation DESC, batch DESC LIMIT 1",
        qualified(writer, PROGRESS_TABLE)
    )
}

/// Record batch `batch` of `generation` as shipped, in the transaction that
/// writes it: `_progress` keeps one row.
pub fn progress_write_sql(writer: &str, generation: i64, batch: i64) -> [String; 2] {
    let progress = qualified(writer, PROGRESS_TABLE);
    [
        format!("DELETE FROM {progress}"),
        format!(
            "INSERT INTO {progress} (generation, batch, shipped_at) \
             VALUES ({generation}, {batch}, now())"
        ),
    ]
}

/// `rows`, in `seq` order, as one [`insert_sql`] per [`INSERT_ROWS`] of them,
/// so no statement grows with its transaction. Each is its own anti-join over
/// its own `seq` range; the caller sends all of them in the batch's one
/// transaction. None when there are no rows.
pub fn insert_statements(
    writer: &str,
    table: &ChangeTable,
    generation: i64,
    rows: &[ShipRow],
) -> Result<Vec<String>, Unrenderable> {
    rows.chunks(INSERT_ROWS)
        .map(|chunk| insert_sql(writer, table, generation, chunk))
        .collect()
}

/// `rows` of `generation` into `table`'s change table, each only if its
/// `(generation, seq)` is not already there: the anti-join is bounded to the
/// batch's `seq` range, which lets DuckLake skip files by their `_seq`
/// statistics. Empty when there are no rows.
pub fn insert_sql(
    writer: &str,
    table: &ChangeTable,
    generation: i64,
    rows: &[ShipRow],
) -> Result<String, Unrenderable> {
    let seqs = rows.iter().map(|r| r.seq);
    let (Some(lo), Some(hi)) = (seqs.clone().min(), seqs.max()) else {
        return Ok(String::new());
    };
    let mut names: Vec<String> = table.columns.iter().map(|c| quote(&c.name)).collect();
    names.extend(RESERVED_COLUMNS.iter().map(|name| quote(name)));
    let names = names.join(", ");
    let values = rows
        .iter()
        .map(|row| row_sql(table, generation, row))
        .collect::<Result<Vec<_>, _>>()?
        .join(",\n  ");
    let target = changes_table(writer, table);
    Ok(format!(
        "INSERT INTO {target} ({names})\nSELECT * FROM (VALUES\n  {values}\n) v({names})\n\
         WHERE NOT EXISTS (SELECT 1 FROM {target} c WHERE c._generation = {generation} \
         AND c._seq BETWEEN {lo} AND {hi} AND c._seq = v._seq)"
    ))
}

/// One row's `( … )`: its attribute values, then its `_` columns.
fn row_sql(table: &ChangeTable, generation: i64, row: &ShipRow) -> Result<String, Unrenderable> {
    if row.generation != generation {
        let reason = format!(
            "of generation {}, in a batch of {generation}",
            row.generation
        );
        return Err(unrenderable(row, "_generation", reason));
    }
    let mut values = attribute_values(table, row)?;
    values.extend(reserved_values(row)?);
    Ok(format!("({})", values.join(", ")))
}

fn unrenderable(row: &ShipRow, column: &str, reason: String) -> Unrenderable {
    Unrenderable {
        seq: row.seq,
        column: column.to_string(),
        reason,
    }
}

/// Each attribute column's literal: the image's value under the plan, or a
/// typed null where the row's shape lacks the column. An image key its shape
/// did not capture is refused, not dropped.
fn attribute_values(table: &ChangeTable, row: &ShipRow) -> Result<Vec<String>, Unrenderable> {
    let mut filled: HashMap<&str, &RawValue> = HashMap::new();
    for (name, value) in row.image.iter().flatten() {
        let Some(column) = table.column(row.schema_version, name) else {
            let reason = format!("not captured under schema_version {}", row.schema_version);
            return Err(unrenderable(row, name, reason));
        };
        filled.insert(column.name.as_str(), &**value);
    }
    table
        .columns
        .iter()
        .map(|column| {
            let value = filled.get(column.name.as_str()).copied();
            literal(value.unwrap_or(RawValue::NULL), &column.ty)
                .map_err(|reason| unrenderable(row, &column.name, reason))
        })
        .collect()
}

/// The `_` columns' literals, in [`RESERVED_COLUMNS`] order.
fn reserved_values(row: &ShipRow) -> Result<[String; 7], Unrenderable> {
    let txid = i64::try_from(row.txid)
        .map_err(|_| unrenderable(row, "_txid", "out of range for BIGINT".to_owned()))?;
    let invocation = row.invocation.map(|id| sql_str(&id.to_string()));
    let changed_at = row.changed_at.to_rfc3339_opts(SecondsFormat::Micros, false);
    Ok([
        cast(&row.generation.to_string(), &DuckType::BigInt),
        cast(&row.seq.to_string(), &DuckType::BigInt),
        cast(&txid.to_string(), &DuckType::BigInt),
        cast(&sql_str(row.op), &DuckType::Varchar),
        cast(&sql_str(&changed_at), &DuckType::TimestampTz),
        cast(&sql_str(&row.actor), &DuckType::Varchar),
        cast(invocation.as_deref().unwrap_or("NULL"), &DuckType::Uuid),
    ])
}

#[cfg(test)]
mod tests;
