//! The app schema's catalog: its tables, their approximate row counts and
//! their columns, read from `pg_catalog` with the schema as a **bound
//! parameter**.
//!
//! The catalog is what every name a request carries is checked against. A
//! table or a column a request names is looked up here by exact match
//! ([`find_table`], [`Table::column`]); only a name the catalog returned is
//! ever written into SQL, and then quoted ([`super::page::quote_ident`]). So a
//! request cannot name another schema, another app's table, or a catalog
//! table: a name that is not one of this schema's tables is not found.
//!
//! Tables only — plain and partitioned, not partitions, views or
//! materialized views — as `oxyc env diff` reads them
//! (`custom_apps_env_branch::diff`): a view is not data the app owns.

use serde::Serialize;
use tokio_postgres::Transaction;

/// What a column holds, as far as rendering a value goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnKind {
    /// A string type (`text`, `varchar`, `char`, `name`, …): shown as text,
    /// cut at [`super::page::TEXT_CHARS`].
    Text,
    /// `json` or `jsonb`: rendered as JSON when it fits, else cut as text.
    Json,
    /// `bytea`: its first [`super::page::BYTES_SHOWN`] bytes in hex and its size.
    Bytes,
    /// Everything else (numbers, timestamps, uuids, arrays, enums, …): its
    /// text form, cut like text.
    Other,
}

/// One column, as the catalog reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Column {
    pub name: String,
    /// `format_type`'s spelling: `text`, `character varying(40)`, `numeric(10,2)`.
    #[serde(rename = "type")]
    pub data_type: String,
    pub kind: ColumnKind,
    pub nullable: bool,
    pub primary_key: bool,
    /// Whether Postgres can `ORDER BY` this column: its type has a default
    /// btree operator class ([`COLUMNS_SQL`]). `json`, `xml`, `point` and a
    /// composite do not, and ordering by one fails with `42883`.
    pub sortable: bool,
}

/// One table of the app's schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Table {
    pub name: String,
    /// `pg_class.reltuples`: Postgres's estimate from the last `ANALYZE` or
    /// vacuum. `None` for a table never analyzed — not "zero rows".
    pub approx_rows: Option<i64>,
    pub columns: Vec<Column>,
}

impl Table {
    /// The column named exactly `name`, if this table has one.
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// The primary key's columns, in column order.
    pub fn primary_key(&self) -> impl Iterator<Item = &Column> {
        self.columns.iter().filter(|c| c.primary_key)
    }
}

/// The table named exactly `name` in `catalog`: no case folding, no
/// qualification, no quoting. `"other_app"."orders"`, `pg_catalog.pg_authid`
/// and `ORDERS` (beside `orders`) are all not found.
pub fn find_table<'a>(catalog: &'a [Table], name: &str) -> Option<&'a Table> {
    catalog.iter().find(|t| t.name == name)
}

/// How a column renders, from its base type (a domain's underlying type)
/// and that type's `typcategory`.
pub fn kind_of(base_type: &str, category: &str) -> ColumnKind {
    match base_type {
        "bytea" => ColumnKind::Bytes,
        "json" | "jsonb" => ColumnKind::Json,
        _ if category == "S" => ColumnKind::Text,
        _ => ColumnKind::Other,
    }
}

/// `reltuples` as a count: negative means "never analyzed" (Postgres 14+).
pub fn approx_rows(reltuples: f64) -> Option<i64> {
    (reltuples >= 0.0).then(|| reltuples.round() as i64)
}

/// One `COLUMNS_SQL` row before it is grouped under its table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawColumn {
    pub table: String,
    pub column: Column,
}

/// Group `columns` under `tables`, both in the order the catalog returned
/// them. A table with no columns keeps an empty list.
pub fn assemble(tables: Vec<(String, f64)>, columns: Vec<RawColumn>) -> Vec<Table> {
    let mut out: Vec<Table> = tables
        .into_iter()
        .map(|(name, reltuples)| Table {
            name,
            approx_rows: approx_rows(reltuples),
            columns: Vec::new(),
        })
        .collect();
    for raw in columns {
        if let Some(table) = out.iter_mut().find(|t| t.name == raw.table) {
            table.columns.push(raw.column);
        }
    }
    out
}

/// The schema's tables, by name. `$1` is the schema.
const TABLES_SQL: &str = "\
    SELECT c.relname::text, c.reltuples::float8 \
      FROM pg_catalog.pg_class c \
      JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
     WHERE n.nspname = $1 AND c.relkind IN ('r', 'p') AND NOT c.relispartition \
     ORDER BY c.relname";

/// The columns of those tables, in table then column order. `$1` is the
/// schema. Dropped columns and system columns are skipped.
///
/// `st` is the column's type with a domain replaced by its base type: what
/// [`kind_of`] reads, and what ordering uses. `x` is the type whose ordering
/// decides `sortable`: `st`, or for an array its element type (an array
/// orders by its elements, through the polymorphic `array_ops`). `x` is
/// sortable when it has a default btree operator class, directly or through
/// an *implicit* binary-coercible cast (`varchar` orders as `text`; `xml`'s
/// cast to `text` is binary but explicit, and Postgres will not order `xml`
/// through it), or is an enum, range or multirange (their polymorphic
/// classes). Anything else — `json`, `xml`,
/// `point`, a composite, a domain over a domain — is reported not sortable:
/// a column wrongly offered fails with `42883`, a column wrongly withheld
/// only loses its sort.
pub const COLUMNS_SQL: &str = "\
    SELECT c.relname::text, a.attname::text, \
           pg_catalog.format_type(a.atttypid, a.atttypmod), a.attnotnull, \
           st.typname::text, st.typcategory::text, \
           EXISTS (SELECT 1 FROM pg_catalog.pg_index i \
                    WHERE i.indrelid = c.oid AND i.indisprimary \
                      AND a.attnum = ANY (i.indkey)), \
           (x.typtype IN ('e', 'r', 'm') \
            OR EXISTS (SELECT 1 FROM pg_catalog.pg_opclass oc \
                         JOIN pg_catalog.pg_am am ON am.oid = oc.opcmethod \
                        WHERE am.amname = 'btree' AND oc.opcdefault \
                          AND (oc.opcintype = x.oid \
                               OR EXISTS (SELECT 1 FROM pg_catalog.pg_cast k \
                                           WHERE k.castsource = x.oid \
                                             AND k.casttarget = oc.opcintype \
                                             AND k.castmethod = 'b' \
                                             AND k.castcontext = 'i')))) \
      FROM pg_catalog.pg_attribute a \
      JOIN pg_catalog.pg_class c ON c.oid = a.attrelid \
      JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
      JOIN pg_catalog.pg_type t ON t.oid = a.atttypid \
      JOIN pg_catalog.pg_type st \
        ON st.oid = CASE WHEN t.typtype = 'd' THEN t.typbasetype ELSE t.oid END \
      JOIN pg_catalog.pg_type x \
        ON x.oid = CASE WHEN st.typcategory = 'A' AND st.typelem <> 0 \
                        THEN st.typelem ELSE st.oid END \
     WHERE n.nspname = $1 AND c.relkind IN ('r', 'p') AND NOT c.relispartition \
       AND a.attnum > 0 AND NOT a.attisdropped \
     ORDER BY c.relname, a.attnum";

/// The catalog of `schema`, read in the caller's read-only transaction.
pub async fn read(
    txn: &Transaction<'_>,
    schema: &str,
) -> Result<Vec<Table>, tokio_postgres::Error> {
    let tables = txn
        .query(TABLES_SQL, &[&schema])
        .await?
        .iter()
        .map(|row| (row.get::<_, String>(0), row.get::<_, f64>(1)))
        .collect();
    let columns = txn
        .query(COLUMNS_SQL, &[&schema])
        .await?
        .iter()
        .map(|row| RawColumn {
            table: row.get(0),
            column: Column {
                name: row.get(1),
                data_type: row.get(2),
                nullable: !row.get::<_, bool>(3),
                kind: kind_of(row.get(4), row.get(5)),
                primary_key: row.get(6),
                sortable: row.get(7),
            },
        })
        .collect();
    Ok(assemble(tables, columns))
}
