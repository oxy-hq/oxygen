//! A declared table's shape, read from the catalog, and whether it can be
//! captured (OLTP change stream design §5.1).
//!
//! The publish reads every declared table's shape with [`SHAPE_SQL`] and
//! [`validate`]s all of them before it changes anything, so a refusal names
//! each table at fault and leaves what is already installed as it was.

use serde::{Deserialize, Serialize};

/// Column names the change table in Airhouse keeps for its own (design
/// §5.4), so a declared table may not have one.
pub const RESERVED_COLUMNS: [&str; 7] = [
    "_generation",
    "_seq",
    "_txid",
    "_op",
    "_changed_at",
    "_actor",
    "_invocation_id",
];

/// One table under `oltpChanges.tables`, as the manifest declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredTable {
    pub name: String,
    /// Columns never captured: in neither the log nor Airhouse.
    pub exclude: Vec<String>,
}

/// A live column. Serialized as a `shapes.columns` element, `{attnum, name,
/// type}`, which is how the relay reads a column's identity through renames.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub attnum: i16,
    pub name: String,
    /// `format_type(atttypid, atttypmod)`, so `numeric(10,2)` and
    /// `varchar(40)` keep their modifiers.
    #[serde(rename = "type")]
    pub type_name: String,
}

/// A table as the catalog has it now: its oid, every live column in
/// `attnum` order, and its primary key in key order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableShape {
    pub relid: u32,
    pub schema: String,
    pub table: String,
    pub columns: Vec<Column>,
    pub primary_key: Vec<String>,
}

/// A shape that passed [`validate`]: the table, and the columns its capture
/// copies (every column but the excluded ones, in `attnum` order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureShape {
    pub shape: TableShape,
    pub captured: Vec<Column>,
}

impl CaptureShape {
    /// The live columns not captured, in `attnum` order: what `shapes.excluded`
    /// records.
    pub fn excluded(&self) -> Vec<String> {
        let mut excluded: Vec<&Column> = self
            .shape
            .columns
            .iter()
            .filter(|c| !self.captured.iter().any(|k| k.name == c.name))
            .collect();
        excluded.sort_by_key(|c| c.attnum);
        excluded.into_iter().map(|c| c.name.clone()).collect()
    }
}

/// One live column of `$1.$2` (schema, table) per row, in `attnum` order:
/// the table's oid, schema and name, the column's number, name and type, and
/// its position in the primary key (null when it is not in it). No rows when
/// no ordinary or partitioned table has that name. Read each row with
/// [`ShapeRow::from_row`].
pub const SHAPE_SQL: &str = "SELECT c.oid AS relid, n.nspname::text AS schema_name, \
       c.relname::text AS table_name, a.attnum, a.attname::text AS column_name, \
       pg_catalog.format_type(a.atttypid, a.atttypmod) AS type_name, \
       pg_catalog.array_position(i.indkey::int2[], a.attnum)::int AS key_position \
  FROM pg_catalog.pg_class c \
  JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
  JOIN pg_catalog.pg_attribute a ON a.attrelid = c.oid \
  LEFT JOIN pg_catalog.pg_index i ON i.indrelid = c.oid AND i.indisprimary \
 WHERE n.nspname = $1 AND c.relname = $2 AND c.relkind IN ('r', 'p') \
   AND a.attnum > 0 AND NOT a.attisdropped \
 ORDER BY a.attnum";

/// One row of [`SHAPE_SQL`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapeRow {
    pub relid: u32,
    pub schema: String,
    pub table: String,
    pub attnum: i16,
    pub name: String,
    pub type_name: String,
    /// 1-based position in the primary key, or `None` outside it.
    pub key_position: Option<i32>,
}

impl ShapeRow {
    /// Read a row of [`SHAPE_SQL`] by its column names.
    pub fn from_row(row: &tokio_postgres::Row) -> Result<Self, tokio_postgres::Error> {
        Ok(Self {
            relid: row.try_get("relid")?,
            schema: row.try_get("schema_name")?,
            table: row.try_get("table_name")?,
            attnum: row.try_get("attnum")?,
            name: row.try_get("column_name")?,
            type_name: row.try_get("type_name")?,
            key_position: row.try_get("key_position")?,
        })
    }
}

/// The shape [`SHAPE_SQL`]'s rows describe, or `None` when there were none:
/// the table does not exist.
pub fn shape_from_rows(rows: &[ShapeRow]) -> Option<TableShape> {
    let first = rows.first()?;
    let mut columns: Vec<Column> = rows
        .iter()
        .map(|r| Column {
            attnum: r.attnum,
            name: r.name.clone(),
            type_name: r.type_name.clone(),
        })
        .collect();
    columns.sort_by_key(|c| c.attnum);
    let mut keyed: Vec<(i32, &str)> = rows
        .iter()
        .filter_map(|r| r.key_position.map(|p| (p, r.name.as_str())))
        .collect();
    keyed.sort_by_key(|(position, _)| *position);
    Some(TableShape {
        relid: first.relid,
        schema: first.schema.clone(),
        table: first.table.clone(),
        columns,
        primary_key: keyed
            .into_iter()
            .map(|(_, name)| name.to_string())
            .collect(),
    })
}

/// Why a declared table cannot be captured. Each names the table and the fix,
/// since it is the app's author who reads it, in the publish's refusal.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeclError {
    #[error(
        "oltpChanges declares table \"{table}\", which does not exist after the app's \
         migrations: create it in a migration, or remove it from oltpChanges.tables"
    )]
    Missing { table: String },
    #[error(
        "oltpChanges table \"{table}\" has no primary key, and every change is keyed by it: \
         add one in a migration, or remove the table from oltpChanges.tables"
    )]
    NoPrimaryKey { table: String },
    #[error(
        "oltpChanges table \"{table}\" has a column named \"{column}\", a name the change \
         table keeps for its own: rename the column in a migration, or remove the table from \
         oltpChanges.tables"
    )]
    ReservedColumn { table: String, column: String },
    #[error(
        "oltpChanges table \"{table}\" excludes \"{column}\", which is not one of its \
         columns: correct the name, or remove it from exclude"
    )]
    UnknownExclude { table: String, column: String },
    #[error(
        "oltpChanges table \"{table}\" excludes \"{column}\", which is part of its primary \
         key, and every change is keyed by it: remove it from exclude"
    )]
    ExcludesKey { table: String, column: String },
}

/// Whether `decl` can be captured as `shape` stands, and if so what its
/// capture copies. The first fault found is the one reported.
pub fn validate(
    decl: &DeclaredTable,
    shape: Option<&TableShape>,
) -> Result<CaptureShape, DeclError> {
    let table = || decl.name.clone();
    let shape = shape.ok_or_else(|| DeclError::Missing { table: table() })?;
    if shape.primary_key.is_empty() {
        return Err(DeclError::NoPrimaryKey { table: table() });
    }
    if let Some(column) = shape
        .columns
        .iter()
        .find(|c| RESERVED_COLUMNS.contains(&c.name.as_str()))
    {
        return Err(DeclError::ReservedColumn {
            table: table(),
            column: column.name.clone(),
        });
    }
    for column in &decl.exclude {
        if !shape.columns.iter().any(|c| &c.name == column) {
            return Err(DeclError::UnknownExclude {
                table: table(),
                column: column.clone(),
            });
        }
        if shape.primary_key.contains(column) {
            return Err(DeclError::ExcludesKey {
                table: table(),
                column: column.clone(),
            });
        }
    }
    let mut captured: Vec<Column> = shape
        .columns
        .iter()
        .filter(|c| !decl.exclude.contains(&c.name))
        .cloned()
        .collect();
    captured.sort_by_key(|c| c.attnum);
    Ok(CaptureShape {
        shape: shape.clone(),
        captured,
    })
}

/// The `schema_version` an install of `capture` records, given the table's
/// latest recorded shape (version, captured columns, key): 1 for a table with
/// none, the same version when the captured columns (number, name and type)
/// and the key are unchanged, and the next one otherwise.
pub fn next_schema_version(
    latest: Option<(i32, &[Column], &[String])>,
    capture: &CaptureShape,
) -> i32 {
    match latest {
        None => 1,
        Some((version, columns, key))
            if columns == capture.captured.as_slice()
                && key == capture.shape.primary_key.as_slice() =>
        {
            version
        }
        Some((version, _, _)) => version + 1,
    }
}

/// `leases (id bigint PRIMARY KEY, name text, contact_email text)` in
/// `app_x`, oid 16423, declared with `contact_email` excluded: the table the
/// unit tests across this module generate SQL for.
#[cfg(test)]
pub(super) fn leases() -> CaptureShape {
    let shape = TableShape {
        relid: 16423,
        schema: "app_x".into(),
        table: "leases".into(),
        columns: vec![
            column(1, "id", "bigint"),
            column(2, "name", "text"),
            column(3, "contact_email", "text"),
        ],
        primary_key: vec!["id".into()],
    };
    let decl = DeclaredTable {
        name: "leases".into(),
        exclude: vec!["contact_email".into()],
    };
    validate(&decl, Some(&shape)).expect("leases is capturable")
}

#[cfg(test)]
pub(super) fn column(attnum: i16, name: &str, type_name: &str) -> Column {
    Column {
        attnum,
        name: name.into(),
        type_name: type_name.into(),
    }
}

#[cfg(test)]
mod tests;
