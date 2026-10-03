//! Seeding a sandbox's schema: a copy of staging's `app_<writer>` on the
//! branch, taken once, when the schema is created.
//!
//! Run **as the app's writer**, on a connection the caller opened on the
//! branch: the writer reads staging's tables (they are its own) and holds
//! `CREATE` on the sandbox's schema, and the copies must be the writer's to
//! `ALTER` — a migration that changes a seeded table needs to own it.
//!
//! **One `REPEATABLE READ` transaction.** Every table is read at one
//! snapshot, so the foreign keys between the copies hold, and a seed that
//! fails leaves nothing behind.
//!
//! **Structure always, rows up to a cap** ([`SeedCaps`]). What a table over a
//! cap gets is its structure and no rows; [`SeedReport::structure_only`] names
//! it.
//!
//! **What is copied:** ordinary and partitioned tables (a partitioned one as a
//! plain table) `LIKE … INCLUDING ALL`; every sequence of the schema, with
//! its position, and each default that named staging's sequence re-pointed to
//! the copy; each foreign key between two tables of the schema, re-pointed.
//! **What is not:** views, functions, triggers, row-security policies, types
//! defined in the schema (a copied column keeps staging's type), and a foreign
//! key to a table in another schema ([`SeedReport::foreign_keys_skipped`]).
//!
//! **What still points at staging's schema** is listed
//! ([`SeedReport::staging_dependencies`]): a column of a type defined there, a
//! default calling a function defined there. Postgres records each as a
//! dependency of the sandbox's table on staging's object, so while the
//! sandbox exists a staging migration that drops that type or function fails
//! ("other objects depend on it") — or, with `CASCADE`, drops the sandbox's
//! column. The list is what explains such a failure.
//!
//! Every identifier in the DDL below is quoted by Postgres itself (`format`'s
//! `%I`), from the catalog's own names: nothing here builds a name by hand.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use tokio_postgres::{Client, IsolationLevel, Transaction};

use super::SandboxSchema;

pub const MAX_ROWS_ENV: &str = "OXY_APP_SANDBOX_OLTP_SEED_MAX_ROWS";
pub const MAX_TABLE_BYTES_ENV: &str = "OXY_APP_SANDBOX_OLTP_SEED_MAX_TABLE_BYTES";
pub const MAX_TOTAL_BYTES_ENV: &str = "OXY_APP_SANDBOX_OLTP_SEED_MAX_TOTAL_BYTES";

/// How much of staging's data a seed copies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeedCaps {
    /// A table with more rows is copied empty.
    pub max_table_rows: i64,
    /// A table bigger than this (heap, indexes and TOAST) is copied empty.
    pub max_table_bytes: i64,
    /// Once the tables copied with rows add up to this, the rest are empty.
    pub max_total_bytes: i64,
}

impl Default for SeedCaps {
    fn default() -> Self {
        Self {
            max_table_rows: 100_000,
            max_table_bytes: 64 * 1024 * 1024,
            max_total_bytes: 256 * 1024 * 1024,
        }
    }
}

impl SeedCaps {
    /// The defaults, each overridden by its env var when that holds a
    /// non-negative whole number.
    pub fn from_env() -> Self {
        let read = |key: &str, default: i64| {
            std::env::var(key)
                .ok()
                .and_then(|v| v.trim().parse::<i64>().ok())
                .filter(|n| *n >= 0)
                .unwrap_or(default)
        };
        let defaults = Self::default();
        Self {
            max_table_rows: read(MAX_ROWS_ENV, defaults.max_table_rows),
            max_table_bytes: read(MAX_TABLE_BYTES_ENV, defaults.max_table_bytes),
            max_total_bytes: read(MAX_TOTAL_BYTES_ENV, defaults.max_total_bytes),
        }
    }
}

/// What a seed copied.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct SeedReport {
    /// Every table copied, in name order.
    pub tables: Vec<String>,
    /// The tables left empty: over a cap.
    pub structure_only: Vec<String>,
    pub rows_copied: u64,
    pub sequences: usize,
    pub foreign_keys: usize,
    /// Added `NOT VALID`: they reference a table left empty.
    pub foreign_keys_not_valid: Vec<String>,
    /// Not copied: they reference a table outside the app's schema.
    pub foreign_keys_skipped: Vec<String>,
    /// What the copies still use of staging's schema — `<column or default>
    /// uses <type or function>` — and so what stops a staging migration from
    /// dropping it while the sandbox exists.
    pub staging_dependencies: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("seeding {schema} failed while it was {step}: {message}")]
pub struct SeedError {
    pub schema: String,
    pub step: &'static str,
    pub message: String,
}

/// One table of staging's schema.
struct SourceTable {
    oid: u32,
    name: String,
    bytes: i64,
    create: String,
}

/// Copy staging's schema into `schema`, which must exist, be empty and be
/// the connected role's to create in. See the module docs.
pub async fn seed(
    client: &mut Client,
    schema: &SandboxSchema,
    caps: &SeedCaps,
) -> Result<SeedReport, SeedError> {
    let run = Run {
        source: schema.app_schema(),
        dest: schema.name(),
    };
    let txn = client
        .build_transaction()
        .isolation_level(IsolationLevel::RepeatableRead)
        .start()
        .await
        .map_err(|e| run.failed("opening its transaction", &e))?;
    // Every name the catalog prints back is then schema-qualified, which is
    // what the re-pointing below matches on.
    txn.batch_execute("SET LOCAL search_path = pg_catalog")
        .await
        .map_err(|e| run.failed("pinning its search path", &e))?;

    let tables = run.tables(&txn).await?;
    let mut report = SeedReport::default();
    for table in &tables {
        exec(&txn, &table.create)
            .await
            .map_err(|e| run.failed("copying a table's structure", &e))?;
        report.tables.push(table.name.clone());
    }
    report.sequences = run.sequences(&txn).await?;
    run.repoint_defaults(&txn).await?;
    run.copy_rows(&txn, &tables, caps, &mut report).await?;
    run.foreign_keys(&txn, &mut report).await?;
    run.sequence_positions(&txn).await?;
    report.staging_dependencies = run.staging_dependencies(&txn).await?;
    txn.commit()
        .await
        .map_err(|e| run.failed("committing", &e))?;
    Ok(report)
}

/// One seed: staging's schema and the sandbox's.
struct Run<'a> {
    source: &'a str,
    dest: &'a str,
}

impl Run<'_> {
    fn failed(&self, step: &'static str, e: &tokio_postgres::Error) -> SeedError {
        SeedError {
            schema: self.dest.to_string(),
            step,
            message: crate::connect::pg_detail(e),
        }
    }

    /// Staging's tables, with the statement that copies each one's structure.
    /// A partition is skipped: its parent is copied as one plain table.
    async fn tables(&self, txn: &Transaction<'_>) -> Result<Vec<SourceTable>, SeedError> {
        let rows = txn
            .query(
                "SELECT c.oid, c.relname::text, \
                        (CASE WHEN c.relkind = 'p' \
                              THEN (SELECT coalesce(sum(pg_total_relation_size(relid)), 0) \
                                      FROM pg_partition_tree(c.oid)) \
                              ELSE pg_total_relation_size(c.oid) END)::bigint, \
                        format('CREATE TABLE %I.%I (LIKE %I.%I INCLUDING ALL)', \
                               $2::text, c.relname, $1::text, c.relname) \
                   FROM pg_class c \
                  WHERE c.relnamespace = (SELECT oid FROM pg_namespace WHERE nspname = $1) \
                    AND c.relkind IN ('r', 'p') AND NOT c.relispartition \
                  ORDER BY c.relname",
                &[&self.source, &self.dest],
            )
            .await
            .map_err(|e| self.failed("listing staging's tables", &e))?;
        Ok(rows
            .iter()
            .map(|row| SourceTable {
                oid: row.get(0),
                name: row.get(1),
                bytes: row.get(2),
                create: row.get(3),
            })
            .collect())
    }

    /// Re-create every sequence of staging's schema that the table copies did
    /// not already bring (an identity column's comes with its column), owned
    /// by the column staging's is owned by.
    async fn sequences(&self, txn: &Transaction<'_>) -> Result<usize, SeedError> {
        let rows = txn
            .query(
                "SELECT format('CREATE SEQUENCE %I.%I AS %s INCREMENT BY %s MINVALUE %s \
                                MAXVALUE %s START WITH %s CACHE %s %s', \
                               $2::text, s.relname, format_type(q.seqtypid, NULL), \
                               q.seqincrement, q.seqmin, q.seqmax, q.seqstart, q.seqcache, \
                               CASE WHEN q.seqcycle THEN 'CYCLE' ELSE 'NO CYCLE' END), \
                        CASE WHEN t.oid IS NULL THEN NULL \
                             ELSE format('ALTER SEQUENCE %I.%I OWNED BY %I.%I.%I', \
                                         $2::text, s.relname, $2::text, t.relname, a.attname) END \
                   FROM pg_class s \
                   JOIN pg_sequence q ON q.seqrelid = s.oid \
                   LEFT JOIN pg_depend d ON d.classid = 'pg_class'::regclass AND d.objid = s.oid \
                        AND d.refclassid = 'pg_class'::regclass AND d.refobjsubid > 0 \
                        AND d.deptype IN ('a', 'i') \
                   LEFT JOIN pg_class t ON t.oid = d.refobjid \
                        AND t.relnamespace = s.relnamespace \
                   LEFT JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = d.refobjsubid \
                  WHERE s.relnamespace = (SELECT oid FROM pg_namespace WHERE nspname = $1) \
                    AND s.relkind = 'S' AND d.deptype IS DISTINCT FROM 'i' \
                  ORDER BY s.relname",
                &[&self.source, &self.dest],
            )
            .await
            .map_err(|e| self.failed("listing staging's sequences", &e))?;
        for row in &rows {
            let create: String = row.get(0);
            exec(txn, &create)
                .await
                .map_err(|e| self.failed("copying a sequence", &e))?;
            if let Some(owned_by) = row.get::<_, Option<String>>(1) {
                exec(txn, &owned_by)
                    .await
                    .map_err(|e| self.failed("giving a sequence its column", &e))?;
            }
        }
        Ok(rows.len())
    }

    /// Point each copied default that reads one of staging's sequences at the
    /// copy of that sequence. A default naming two sequences is rewritten
    /// once, with both.
    async fn repoint_defaults(&self, txn: &Transaction<'_>) -> Result<(), SeedError> {
        let rows = txn
            .query(
                "SELECT format('ALTER TABLE %I.%I ALTER COLUMN %I SET DEFAULT ', \
                               $2::text, t.relname, a.attname), \
                        pg_get_expr(ad.adbin, ad.adrelid), \
                        quote_literal(format('%I.%I', $1::text, s.relname)) || '::regclass', \
                        quote_literal(format('%I.%I', $2::text, s.relname)) || '::regclass' \
                   FROM pg_attrdef ad \
                   JOIN pg_class t ON t.oid = ad.adrelid \
                   JOIN pg_attribute a ON a.attrelid = ad.adrelid AND a.attnum = ad.adnum \
                   JOIN pg_depend d ON d.classid = 'pg_attrdef'::regclass AND d.objid = ad.oid \
                        AND d.refclassid = 'pg_class'::regclass \
                   JOIN pg_class s ON s.oid = d.refobjid AND s.relkind = 'S' \
                  WHERE t.relnamespace = (SELECT oid FROM pg_namespace WHERE nspname = $2) \
                    AND s.relnamespace = (SELECT oid FROM pg_namespace WHERE nspname = $1) \
                    AND a.attgenerated = ''",
                &[&self.source, &self.dest],
            )
            .await
            .map_err(|e| self.failed("reading the copied defaults", &e))?;
        let mut defaults: BTreeMap<String, String> = BTreeMap::new();
        for row in &rows {
            let (alter, expr): (String, String) = (row.get(0), row.get(1));
            let (from, to): (String, String) = (row.get(2), row.get(3));
            let expr = defaults.entry(alter).or_insert(expr);
            *expr = expr.replace(&from, &to);
        }
        for (alter, expr) in defaults {
            exec(txn, &format!("{alter}{expr}"))
                .await
                .map_err(|e| self.failed("re-pointing a default to the copied sequence", &e))?;
        }
        Ok(())
    }

    /// Copy each table's rows while it, and the sandbox so far, stay under
    /// the caps. A table over one is left empty and named in the report.
    async fn copy_rows(
        &self,
        txn: &Transaction<'_>,
        tables: &[SourceTable],
        caps: &SeedCaps,
        report: &mut SeedReport,
    ) -> Result<(), SeedError> {
        let mut total = 0_i64;
        for table in tables {
            let fits = table.bytes <= caps.max_table_bytes
                && total.saturating_add(table.bytes) <= caps.max_total_bytes
                && self.within_row_cap(txn, table, caps.max_table_rows).await?;
            if !fits {
                report.structure_only.push(table.name.clone());
                continue;
            }
            let Some(insert) = self.insert_statement(txn, table).await? else {
                continue;
            };
            let copied = txn
                .execute(insert.as_str(), &[])
                .await
                .map_err(|e| self.failed("copying a table's rows", &e))?;
            report.rows_copied += copied;
            total = total.saturating_add(table.bytes);
        }
        Ok(())
    }

    /// Whether the table holds at most `cap` rows, counted no further than
    /// one past it.
    async fn within_row_cap(
        &self,
        txn: &Transaction<'_>,
        table: &SourceTable,
        cap: i64,
    ) -> Result<bool, SeedError> {
        let count = txn
            .query_one(
                "SELECT format('SELECT count(*) FROM (SELECT 1 FROM %I.%I LIMIT %s) bounded', \
                               $1::text, $2::text, $3::bigint + 1)",
                &[&self.source, &table.name, &cap],
            )
            .await
            .map_err(|e| self.failed("building a row count", &e))?
            .get::<_, String>(0);
        let rows: i64 = txn
            .query_one(count.as_str(), &[])
            .await
            .map_err(|e| self.failed("counting a table's rows", &e))?
            .get(0);
        Ok(rows <= cap)
    }

    /// `INSERT … SELECT` of every column a row can be given: a generated
    /// column computes itself, and an identity column takes the copied value
    /// (`OVERRIDING SYSTEM VALUE`). `None` for a table with no such column.
    async fn insert_statement(
        &self,
        txn: &Transaction<'_>,
        table: &SourceTable,
    ) -> Result<Option<String>, SeedError> {
        let row = txn
            .query_one(
                "SELECT CASE WHEN cols IS NULL THEN NULL ELSE \
                        format('INSERT INTO %I.%I (%s) OVERRIDING SYSTEM VALUE \
                                SELECT %s FROM %I.%I', \
                               $2::text, $3::text, cols, cols, $1::text, $3::text) END \
                   FROM (SELECT string_agg(quote_ident(attname), ', ' ORDER BY attnum) AS cols \
                           FROM pg_attribute \
                          WHERE attrelid = $4 AND attnum > 0 AND NOT attisdropped \
                            AND attgenerated = '') columns",
                &[&self.source, &self.dest, &table.name, &table.oid],
            )
            .await
            .map_err(|e| self.failed("listing a table's columns", &e))?;
        Ok(row.get(0))
    }

    /// Add each foreign key between two of the copied tables, pointing at the
    /// copies; `NOT VALID` when the table it references was left empty. One
    /// that references another schema is not copied.
    async fn foreign_keys(
        &self,
        txn: &Transaction<'_>,
        report: &mut SeedReport,
    ) -> Result<(), SeedError> {
        let rows = txn
            .query(
                "SELECT con.conname::text, child.relname::text, parent.relname::text, \
                        pn.nspname = $1, con.convalidated, \
                        format('ALTER TABLE %I.%I ADD CONSTRAINT %I %s', \
                               $2::text, child.relname, con.conname, \
                               replace(pg_get_constraintdef(con.oid), \
                                       'REFERENCES ' || format('%I.%I', $1::text, parent.relname) \
                                           || '(', \
                                       'REFERENCES ' || format('%I.%I', $2::text, parent.relname) \
                                           || '(')) \
                   FROM pg_constraint con \
                   JOIN pg_class child ON child.oid = con.conrelid \
                   JOIN pg_class parent ON parent.oid = con.confrelid \
                   JOIN pg_namespace pn ON pn.oid = parent.relnamespace \
                  WHERE con.contype = 'f' AND con.conparentid = 0 AND NOT child.relispartition \
                    AND child.relnamespace = (SELECT oid FROM pg_namespace WHERE nspname = $1) \
                  ORDER BY child.relname, con.conname",
                &[&self.source, &self.dest],
            )
            .await
            .map_err(|e| self.failed("listing staging's foreign keys", &e))?;
        let empty: BTreeSet<&str> = report.structure_only.iter().map(String::as_str).collect();
        let mut not_valid = Vec::new();
        let mut skipped = Vec::new();
        let mut added = 0;
        for row in &rows {
            let (name, child, parent): (String, String, String) =
                (row.get(0), row.get(1), row.get(2));
            let label = format!("{child}.{name}");
            if !row.get::<_, bool>(3) {
                skipped.push(label);
                continue;
            }
            let mut add: String = row.get(5);
            // Staging's own `NOT VALID` is already in the definition.
            let validated: bool = row.get(4);
            if validated && empty.contains(parent.as_str()) && !empty.contains(child.as_str()) {
                add.push_str(" NOT VALID");
                not_valid.push(label);
            }
            exec(txn, &add)
                .await
                .map_err(|e| self.failed("copying a foreign key", &e))?;
            added += 1;
        }
        report.foreign_keys = added;
        report.foreign_keys_not_valid = not_valid;
        report.foreign_keys_skipped = skipped;
        Ok(())
    }

    /// Every object of staging's schema that a copied column, default or
    /// constraint still depends on: types, domains, functions. Sequences and
    /// tables are re-pointed above, so they do not appear.
    async fn staging_dependencies(&self, txn: &Transaction<'_>) -> Result<Vec<String>, SeedError> {
        let rows = txn
            .query(
                "SELECT DISTINCT format('%s uses %s %s', \
                        (pg_identify_object(d.classid, d.objid, d.objsubid)).identity, \
                        referenced.type, referenced.identity) \
                   FROM pg_depend d \
                  CROSS JOIN LATERAL pg_identify_object(d.refclassid, d.refobjid, 0) referenced \
                  WHERE d.deptype = 'n' AND referenced.schema = $1 \
                    AND ((d.classid = 'pg_class'::regclass AND d.objid IN \
                            (SELECT oid FROM pg_class WHERE relnamespace = \
                               (SELECT oid FROM pg_namespace WHERE nspname = $2))) \
                      OR (d.classid = 'pg_attrdef'::regclass AND d.objid IN \
                            (SELECT ad.oid FROM pg_attrdef ad JOIN pg_class c ON c.oid = ad.adrelid \
                              WHERE c.relnamespace = \
                                (SELECT oid FROM pg_namespace WHERE nspname = $2))) \
                      OR (d.classid = 'pg_constraint'::regclass AND d.objid IN \
                            (SELECT oid FROM pg_constraint WHERE connamespace = \
                               (SELECT oid FROM pg_namespace WHERE nspname = $2)))) \
                  ORDER BY 1",
                &[&self.source, &self.dest],
            )
            .await
            .map_err(|e| self.failed("listing what still points at staging's schema", &e))?;
        Ok(rows.iter().map(|row| row.get(0)).collect())
    }

    /// Put each copied sequence where staging's is, so a row inserted in the
    /// sandbox does not take an id a copied row holds. A sequence staging
    /// never advanced is left at its start.
    async fn sequence_positions(&self, txn: &Transaction<'_>) -> Result<(), SeedError> {
        let rows = txn
            .query(
                "SELECT format('SELECT setval(%L, %s, true)', \
                               CASE WHEN d.deptype = 'i' \
                                    THEN pg_get_serial_sequence( \
                                             format('%I.%I', $2::text, t.relname), a.attname) \
                                    ELSE format('%I.%I', $2::text, ps.sequencename) END, \
                               ps.last_value) \
                   FROM pg_sequences ps \
                   JOIN pg_class s ON s.relname = ps.sequencename AND s.relkind = 'S' \
                        AND s.relnamespace = (SELECT oid FROM pg_namespace WHERE nspname = $1) \
                   LEFT JOIN pg_depend d ON d.classid = 'pg_class'::regclass AND d.objid = s.oid \
                        AND d.refclassid = 'pg_class'::regclass AND d.refobjsubid > 0 \
                        AND d.deptype IN ('a', 'i') \
                   LEFT JOIN pg_class t ON t.oid = d.refobjid \
                   LEFT JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = d.refobjsubid \
                  WHERE ps.schemaname = $1 AND ps.last_value IS NOT NULL",
                &[&self.source, &self.dest],
            )
            .await
            .map_err(|e| self.failed("reading staging's sequence positions", &e))?;
        for row in &rows {
            // `None`: an identity column whose copy has no sequence to set.
            let Some(setval) = row.get::<_, Option<String>>(0) else {
                continue;
            };
            txn.query(setval.as_str(), &[])
                .await
                .map_err(|e| self.failed("positioning a copied sequence", &e))?;
        }
        Ok(())
    }
}

async fn exec(txn: &Transaction<'_>, sql: &str) -> Result<(), tokio_postgres::Error> {
    txn.batch_execute(sql).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_caps_are_the_documented_ones() {
        let caps = SeedCaps::default();
        assert_eq!(caps.max_table_rows, 100_000);
        assert_eq!(caps.max_table_bytes, 64 * 1024 * 1024);
        assert_eq!(caps.max_total_bytes, 256 * 1024 * 1024);
    }
}
