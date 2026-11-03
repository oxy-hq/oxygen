//! The OLTP schema difference between a sandbox's own branch and production
//! (`oxyc env diff`): what the sandbox's migrations changed, as a list a
//! person can read before promoting.
//!
//! [`diff`] is pure: two column catalogs in, one [`OltpDiff`] out. The catalogs
//! come from [`read_catalog`], which reads `pg_catalog` for the app's writer
//! schema inside a `READ ONLY` transaction, so neither side — production above
//! all — can be written by looking at it.
//!
//! **Types are `format_type`'s**, not `information_schema`'s `data_type`: the
//! latter says `character varying` for every varchar length, `numeric` for any
//! precision and `ARRAY` for every array, which would call a narrowed column
//! "no difference".
//!
//! **Columns are compared on tables both sides have.** A table that is new on
//! the branch is one line under `tables_added`, not a line per column.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use tokio_postgres::Client;

/// One column as the catalog reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogColumn {
    pub table: String,
    pub column: String,
    /// `format_type`'s full spelling: `text`, `character varying(40)`,
    /// `numeric(10,2)`, `integer[]`, an enum's name, …
    pub data_type: String,
    pub nullable: bool,
}

impl CatalogColumn {
    /// `type`, then `not null` when the column cannot hold NULL.
    fn describe(&self) -> String {
        if self.nullable {
            self.data_type.clone()
        } else {
            format!("{} not null", self.data_type)
        }
    }

    fn qualified(&self) -> String {
        format!("{}.{}", self.table, self.column)
    }
}

/// How the branch's schema differs from production's. Every list is sorted;
/// all empty means the schemas match.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct OltpDiff {
    /// The app's writer schema, which both sides were read in.
    pub schema: String,
    pub tables_added: Vec<String>,
    pub tables_removed: Vec<String>,
    /// `table.column type` (plus `not null`), on a table both sides have.
    pub columns_added: Vec<String>,
    pub columns_removed: Vec<String>,
    /// `table.column: <production> → <branch>` for a type or nullability change.
    pub columns_changed: Vec<String>,
}

type ByKey<'a> = BTreeMap<(&'a str, &'a str), &'a CatalogColumn>;

fn by_key(columns: &[CatalogColumn]) -> ByKey<'_> {
    columns
        .iter()
        .map(|c| ((c.table.as_str(), c.column.as_str()), c))
        .collect()
}

fn tables(columns: &[CatalogColumn]) -> BTreeSet<&str> {
    columns.iter().map(|c| c.table.as_str()).collect()
}

/// `branch` against `production`: the tables and columns the branch gained,
/// lost and changed. `schema` is left empty: the caller, who read the
/// catalogs from it, says which schema that was.
pub fn diff(production: &[CatalogColumn], branch: &[CatalogColumn]) -> OltpDiff {
    let (prod_tables, branch_tables) = (tables(production), tables(branch));
    let named = |set: BTreeSet<&str>| set.into_iter().map(str::to_string).collect();
    let (prod, side) = (by_key(production), by_key(branch));
    let shared = |table: &str| prod_tables.contains(table) && branch_tables.contains(table);
    let mut out = OltpDiff {
        tables_added: named(branch_tables.difference(&prod_tables).copied().collect()),
        tables_removed: named(prod_tables.difference(&branch_tables).copied().collect()),
        ..OltpDiff::default()
    };
    for (key, column) in &side {
        match prod.get(key) {
            None if shared(key.0) => {
                out.columns_added
                    .push(format!("{} {}", column.qualified(), column.describe()))
            }
            Some(before) if before.describe() != column.describe() => {
                out.columns_changed.push(format!(
                    "{}: {} → {}",
                    column.qualified(),
                    before.describe(),
                    column.describe()
                ))
            }
            _ => {}
        }
    }
    for (key, column) in &prod {
        if !side.contains_key(key) && shared(key.0) {
            out.columns_removed.push(column.qualified());
        }
    }
    out
}

/// The columns of the tables (plain and partitioned) of `schema`, read in a
/// `READ ONLY` transaction. Views are left out: a view is not data the app
/// owns. Dropped columns are skipped.
const COLUMNS_SQL: &str = "\
    SELECT c.relname::text, a.attname::text, \
           pg_catalog.format_type(a.atttypid, a.atttypmod), a.attnotnull \
      FROM pg_catalog.pg_attribute a \
      JOIN pg_catalog.pg_class c ON c.oid = a.attrelid \
      JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
     WHERE n.nspname = $1 AND c.relkind IN ('r', 'p') \
       AND a.attnum > 0 AND NOT a.attisdropped \
     ORDER BY c.relname, a.attnum";

pub async fn read_catalog(
    client: &mut Client,
    schema: &str,
) -> Result<Vec<CatalogColumn>, tokio_postgres::Error> {
    let txn = client.build_transaction().read_only(true).start().await?;
    let rows = txn.query(COLUMNS_SQL, &[&schema]).await?;
    txn.rollback().await?;
    Ok(rows
        .iter()
        .map(|row| CatalogColumn {
            table: row.get(0),
            column: row.get(1),
            data_type: row.get(2),
            nullable: !row.get::<_, bool>(3),
        })
        .collect())
}

#[cfg(test)]
#[path = "diff_tests.rs"]
mod tests;
