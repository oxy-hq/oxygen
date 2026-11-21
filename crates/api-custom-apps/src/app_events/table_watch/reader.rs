//! How the tracker reads DuckLake (app events design §7.2, step 2): the
//! snapshot range, the table's columns, which snapshots changed it, and what
//! each changed.
//!
//! One door, [`TableReader`], so a test installs a fake for all of it
//! ([`install_reader`]); production's [`AirhouseTableReader`] connects as a
//! **Reader** the platform mints for the workspace
//! ([`airhouse::SystemPurpose::TableWatch`]) — never an app's credential, and
//! no write.
//!
//! **What counts as a change** is decided from the snapshot's recorded change
//! kind, never from `table_changes` alone ([`changed_snapshots`]): DuckLake's
//! compactor writes snapshots too, and some of them (a rewrite of data files
//! that carry deletes) come back from `table_changes` as an update of every
//! surviving row. See `internal-docs/customer-apps-functions.md`, "Table
//! changes", for the evidence.
//!
//! **Rows are never filtered by `change_type` in SQL.** On Airhouse's DuckLake
//! any such filter on `ducklake_table_changes` fails inside DuckDB
//! ("INTERNAL Error: optional_idx cannot be initialized with an invalid
//! index"), so the reader counts with `GROUP BY change_type`, orders every
//! inserted row before every deleted one ([`SIDE_ORDER`]), and splits the
//! sides itself. Airhouse's `tests/change_feed_reader.rs` runs these
//! statements verbatim and pins that failure.
//!
//! The credential is minted with `uncached_reads`, so a commit made through
//! any Airhouse pod is in the next pass rather than up to a pod's cache TTL
//! later.

use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock, RwLock};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use oxy_custom_apps::events::table::{ChangePage, Changed, INLINE_MAX_ROWS, Row};
use serde_json::Value;
use tokio_postgres::SimpleQueryMessage;
use uuid::Uuid;

/// The DuckLake catalog's name when the deployment does not say
/// (`AIRHOUSE_CATALOG`): Airhouse's own default.
const DEFAULT_CATALOG: &str = "lake";
/// The metadata columns `table_changes` adds to a table's own.
const META_COLUMNS: [&str; 3] = ["snapshot_id", "rowid", "change_type"];
/// The longest one read may take.
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Why a read did not answer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReadError {
    /// The workspace has no Airhouse (or the deployment has none): nothing to
    /// watch, and no failure either.
    #[error("the workspace has no Airhouse")]
    NotConfigured,
    /// The snapshot (or the range from it) has been expired.
    #[error("snapshot expired: {0}")]
    SnapshotGone(String),
    /// The table or schema is not there.
    #[error("table missing: {0}")]
    TableMissing(String),
    #[error("{0}")]
    Failed(String),
}

/// The oldest and newest snapshot DuckLake still holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotRange {
    pub oldest: i64,
    pub newest: i64,
}

/// A table's columns: name to type, in name order.
pub type Columns = BTreeMap<String, String>;

#[async_trait]
pub trait TableReader: Send + Sync {
    /// The snapshots DuckLake holds. `Err(NotConfigured)` for a workspace
    /// with no Airhouse.
    async fn snapshots(&self, workspace_id: Uuid) -> Result<SnapshotRange, ReadError>;

    /// The newest snapshot taken at or before `at`, if any.
    async fn snapshot_at(
        &self,
        workspace_id: Uuid,
        at: DateTime<Utc>,
    ) -> Result<Option<i64>, ReadError>;

    /// The catalog id of `schema.table` now; `None` when it does not exist. A
    /// `CREATE OR REPLACE` or a drop and create gives the name a new id.
    async fn table_id(
        &self,
        workspace_id: Uuid,
        schema: &str,
        table: &str,
    ) -> Result<Option<i64>, ReadError>;

    /// `schema.table`'s columns; `None` when it does not exist.
    async fn columns(
        &self,
        workspace_id: Uuid,
        schema: &str,
        table: &str,
    ) -> Result<Option<Columns>, ReadError>;

    /// The snapshots in `(after, upto]` that inserted into or deleted from
    /// the table, oldest first. Compaction is not among them.
    async fn changed_snapshots(
        &self,
        workspace_id: Uuid,
        schema: &str,
        table: &str,
        after: i64,
        upto: i64,
    ) -> Result<Vec<i64>, ReadError>;

    /// What `snapshot` changed: the counts, and the rows when at most
    /// [`INLINE_MAX_ROWS`].
    async fn changes_at(
        &self,
        workspace_id: Uuid,
        schema: &str,
        table: &str,
        snapshot: i64,
    ) -> Result<Changed, ReadError>;

    /// One page of what `snapshot` changed, each side from `offset`.
    async fn change_page(
        &self,
        workspace_id: Uuid,
        schema: &str,
        table: &str,
        snapshot: i64,
        limit: usize,
        offset: usize,
    ) -> Result<ChangePage, ReadError>;
}

static READER: LazyLock<RwLock<Arc<dyn TableReader>>> =
    LazyLock::new(|| RwLock::new(Arc::new(AirhouseTableReader)));

/// Replace the process's reader. For tests, which have no Airhouse.
pub fn install_reader(reader: Arc<dyn TableReader>) {
    *READER.write().unwrap_or_else(|e| e.into_inner()) = reader;
}

/// The process's reader.
pub fn reader() -> Arc<dyn TableReader> {
    READER.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Which snapshot kinds of `changes_made` (`inserted_into_table:3`,
/// `inlined_delete:3`, ...) change a table's rows.
const ROW_CHANGES: [&str; 4] = [
    "inserted_into_table",
    "deleted_from_table",
    "inlined_insert",
    "inlined_delete",
];

/// Split `changes_made` into its entries: commas separate them, except inside
/// the double quotes of a name (`created_table:"s"."a,b"`).
fn entries(changes_made: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut start, mut quoted) = (0, false);
    for (i, c) in changes_made.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => {
                out.push(&changes_made[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&changes_made[start..]);
    out
}

/// Whether a snapshot's recorded `changes_made` inserts into or deletes from
/// the table with id `table_id`. Compaction (`merge_adjacent`,
/// `rewrite_delete`, `inline_flush`, ...) and DDL are not row changes.
pub fn changes_rows_of(changes_made: &str, table_id: i64) -> bool {
    entries(changes_made).into_iter().any(|entry| {
        entry.split_once(':').is_some_and(|(kind, id)| {
            ROW_CHANGES.contains(&kind) && id.trim().parse::<i64>() == Ok(table_id)
        })
    })
}

/// A column list's contract difference: what a listener relied on that is no
/// longer so. A column added is no difference.
pub fn broken_columns(contract: &Columns, now: &Columns) -> Vec<String> {
    let mut broken = Vec::new();
    for (name, kind) in contract {
        match now.get(name) {
            None => broken.push(format!("column `{name}` is gone (removed or renamed)")),
            Some(now_kind) if now_kind != kind => {
                broken.push(format!(
                    "column `{name}` changed type, {kind} to {now_kind}"
                ));
            }
            Some(_) => {}
        }
    }
    for name in now.keys().filter(|n| META_COLUMNS.contains(&n.as_str())) {
        broken.push(format!(
            "column `{name}` clashes with a column table_changes adds ({})",
            META_COLUMNS.join(", ")
        ));
    }
    broken
}

fn quote(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

fn catalog() -> String {
    airhouse::ducklake_catalog().unwrap_or_else(|| DEFAULT_CATALOG.to_string())
}

/// `ducklake_table_changes('<catalog>', '<schema>', '<table>', S, S)`: the form
/// that works for any schema (the `<catalog>.<schema>.table_changes` form fails
/// outside `main`). Only ever a single snapshot, so a compaction snapshot the
/// native compactor wrote can never be inside the range.
fn changes_fn(schema: &str, table: &str, snapshot: i64) -> String {
    format!(
        "ducklake_table_changes({}, {}, {}, {snapshot}, {snapshot})",
        literal(&catalog()),
        literal(schema),
        literal(table)
    )
}

/// Orders a snapshot's changed rows with every inserted row (`insert`,
/// `update_postimage`) before every deleted one (`delete`,
/// `update_preimage`), each side by `rowid`. A side is then one contiguous
/// range, read with `LIMIT`/`OFFSET` from the counts, with no filter on
/// `change_type` (see the module doc).
const SIDE_ORDER: &str = "CASE WHEN change_type IN ('insert', 'update_postimage') THEN 0 ELSE 1 END, \
     rowid, change_type";

/// What a snapshot changed, from `SELECT change_type, count(*) … GROUP BY
/// change_type`. An update is counted on both sides and as an update.
fn tally(counts: Vec<Vec<Option<String>>>) -> Changed {
    let mut changed = Changed::default();
    for row in counts {
        let n: u64 = row
            .get(1)
            .and_then(|c| c.as_deref())
            .and_then(|c| c.trim().parse().ok())
            .unwrap_or(0);
        match row.first().and_then(|c| c.as_deref()) {
            Some("insert") => changed.inserted += n,
            Some("update_postimage") => {
                changed.inserted += n;
                changed.updated += n;
            }
            Some("delete" | "update_preimage") => changed.deleted += n,
            _ => {}
        }
    }
    changed
}

/// Split changed rows into `(inserted, deleted)` by each row's `change_type`.
fn split_sides(rows: Vec<Vec<Option<String>>>) -> Result<(Vec<Row>, Vec<Row>), ReadError> {
    let (mut inserted, mut deleted) = (Vec::new(), Vec::new());
    for text in rows.into_iter().filter_map(|mut r| r.pop().flatten()) {
        let (change_type, row) = parse_change(&text)?;
        match change_type.as_str() {
            "insert" | "update_postimage" => inserted.push(row),
            "delete" | "update_preimage" => deleted.push(row),
            other => {
                return Err(ReadError::Failed(format!(
                    "a changed row has an unknown change_type `{other}`"
                )));
            }
        }
    }
    Ok((inserted, deleted))
}

/// Production's reader: a Reader the platform mints for the workspace.
pub struct AirhouseTableReader;

type Messages = Vec<SimpleQueryMessage>;

fn text_rows(messages: Messages) -> Vec<Vec<Option<String>>> {
    messages
        .into_iter()
        .filter_map(|m| match m {
            SimpleQueryMessage::Row(row) => Some(
                (0..row.len())
                    .map(|i| row.get(i).map(str::to_string))
                    .collect(),
            ),
            _ => None,
        })
        .collect()
}

fn classify(text: String) -> ReadError {
    let lower = text.to_lowercase();
    if lower.contains("no snapshot found") {
        ReadError::SnapshotGone(text)
    } else if lower.contains("does not exist") || lower.contains("not found") {
        ReadError::TableMissing(text)
    } else {
        ReadError::Failed(text)
    }
}

impl AirhouseTableReader {
    /// `NotConfigured` for a workspace with no active Airhouse tenant: the
    /// tracker passes it by, and a resync answers it plainly.
    async fn has_tenant(workspace_id: Uuid) -> Result<(), ReadError> {
        let db = oxy::database::client::establish_connection()
            .await
            .map_err(|e| ReadError::Failed(e.to_string()))?;
        match crate::app_events::mirror::workspace_has_airhouse(&db, workspace_id).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(ReadError::NotConfigured),
            Err(e) => Err(ReadError::Failed(e.to_string())),
        }
    }

    async fn run(
        &self,
        workspace_id: Uuid,
        sql: &str,
    ) -> Result<Vec<Vec<Option<String>>>, ReadError> {
        Self::has_tenant(workspace_id).await?;
        let client = oxy_cameras::airhouse::client::tenant_client(
            workspace_id,
            airhouse::UserRole::Reader,
            airhouse::SystemPurpose::TableWatch,
            oxy_cameras::airhouse::read_ttl(),
            oxy_cameras::airhouse::client::Pool::Analytics,
        )
        .await
        .map_err(|e| ReadError::Failed(e.to_string()))?;
        let answered = tokio::time::timeout(READ_TIMEOUT, client.simple_query(sql))
            .await
            .map_err(|_| ReadError::Failed("the Airhouse read timed out".into()))?;
        match answered {
            Ok(messages) => Ok(text_rows(messages)),
            Err(e) => Err(classify(match e.as_db_error() {
                Some(db) => db.message().to_string(),
                None => e.to_string(),
            })),
        }
    }

    async fn first_number(&self, workspace_id: Uuid, sql: &str) -> Result<Option<i64>, ReadError> {
        let rows = self.run(workspace_id, sql).await?;
        Ok(rows
            .first()
            .and_then(|r| r.first())
            .and_then(|c| c.as_deref())
            .and_then(|c| c.trim().parse().ok()))
    }

    async fn table_id(
        &self,
        workspace_id: Uuid,
        schema: &str,
        table: &str,
    ) -> Result<Option<i64>, ReadError> {
        let meta = quote(&format!("__ducklake_metadata_{}", catalog()));
        self.first_number(
            workspace_id,
            &format!(
                "SELECT t.table_id FROM {meta}.ducklake_table t \
                 JOIN {meta}.ducklake_schema s ON s.schema_id = t.schema_id \
                 WHERE s.schema_name = {} AND t.table_name = {} \
                   AND t.end_snapshot IS NULL AND s.end_snapshot IS NULL",
                literal(schema),
                literal(table)
            ),
        )
        .await
    }
}

/// A changed row's `change_type`, and the row without the metadata columns
/// `table_changes` adds.
fn parse_change(text: &str) -> Result<(String, Row), ReadError> {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(mut row)) => {
            let change_type = match row.get("change_type") {
                Some(Value::String(t)) => t.clone(),
                _ => {
                    return Err(ReadError::Failed("a changed row has no change_type".into()));
                }
            };
            for meta in META_COLUMNS {
                row.remove(meta);
            }
            Ok((change_type, row))
        }
        _ => Err(ReadError::Failed(
            "a changed row did not read as JSON".into(),
        )),
    }
}

#[async_trait]
impl TableReader for AirhouseTableReader {
    async fn snapshots(&self, workspace_id: Uuid) -> Result<SnapshotRange, ReadError> {
        let lake = quote(&catalog());
        // The head from the catalog's current snapshot, never a per-connection
        // "last committed"; the oldest from the retained list.
        let rows = self
            .run(
                workspace_id,
                &format!(
                    "SELECT (SELECT min(snapshot_id) FROM {lake}.snapshots()), \
                            (SELECT id FROM ducklake_current_snapshot({}))",
                    literal(&catalog())
                ),
            )
            .await?;
        let number = |i: usize| {
            rows.first()
                .and_then(|r| r.get(i))
                .and_then(|c| c.as_deref())
                .and_then(|c| c.trim().parse::<i64>().ok())
        };
        match (number(0), number(1)) {
            (Some(oldest), Some(newest)) => Ok(SnapshotRange { oldest, newest }),
            _ => Err(ReadError::Failed("the catalog has no snapshots".into())),
        }
    }

    async fn snapshot_at(
        &self,
        workspace_id: Uuid,
        at: DateTime<Utc>,
    ) -> Result<Option<i64>, ReadError> {
        let lake = quote(&catalog());
        self.first_number(
            workspace_id,
            &format!(
                "SELECT max(snapshot_id) FROM {lake}.snapshots() \
                 WHERE snapshot_time <= TIMESTAMPTZ {}",
                literal(&at.format("%Y-%m-%d %H:%M:%S%.6f+00").to_string())
            ),
        )
        .await
    }

    async fn table_id(
        &self,
        workspace_id: Uuid,
        schema: &str,
        table: &str,
    ) -> Result<Option<i64>, ReadError> {
        AirhouseTableReader::table_id(self, workspace_id, schema, table).await
    }

    async fn columns(
        &self,
        workspace_id: Uuid,
        schema: &str,
        table: &str,
    ) -> Result<Option<Columns>, ReadError> {
        // From the catalog's own tables, which are never cached
        // (`information_schema.columns` is, for a minute, per pod).
        let meta = quote(&format!("__ducklake_metadata_{}", catalog()));
        let rows = self
            .run(
                workspace_id,
                &format!(
                    "SELECT c.column_name, c.column_type FROM {meta}.ducklake_column c \
                     JOIN {meta}.ducklake_table t ON t.table_id = c.table_id \
                     JOIN {meta}.ducklake_schema s ON s.schema_id = t.schema_id \
                     WHERE s.schema_name = {} AND t.table_name = {} \
                       AND s.end_snapshot IS NULL AND t.end_snapshot IS NULL \
                       AND c.end_snapshot IS NULL AND c.parent_column IS NULL \
                     ORDER BY c.column_order",
                    literal(schema),
                    literal(table)
                ),
            )
            .await?;
        let columns: Columns = rows
            .into_iter()
            .filter_map(|r| Some((r.first()?.clone()?, r.get(1)?.clone()?)))
            .collect();
        Ok((!columns.is_empty()).then_some(columns))
    }

    async fn changed_snapshots(
        &self,
        workspace_id: Uuid,
        schema: &str,
        table: &str,
        after: i64,
        upto: i64,
    ) -> Result<Vec<i64>, ReadError> {
        let Some(table_id) = self.table_id(workspace_id, schema, table).await? else {
            return Err(ReadError::TableMissing(format!("{schema}.{table}")));
        };
        let meta = quote(&format!("__ducklake_metadata_{}", catalog()));
        let rows = self
            .run(
                workspace_id,
                &format!(
                    "SELECT snapshot_id, changes_made FROM {meta}.ducklake_snapshot_changes \
                     WHERE snapshot_id > {after} AND snapshot_id <= {upto} ORDER BY snapshot_id"
                ),
            )
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let id = r.first()?.as_deref()?.trim().parse::<i64>().ok()?;
                changes_rows_of(r.get(1)?.as_deref()?, table_id).then_some(id)
            })
            .collect())
    }

    async fn changes_at(
        &self,
        workspace_id: Uuid,
        schema: &str,
        table: &str,
        snapshot: i64,
    ) -> Result<Changed, ReadError> {
        let from = changes_fn(schema, table, snapshot);
        let mut changed = tally(
            self.run(
                workspace_id,
                &format!("SELECT change_type, count(*) FROM {from} GROUP BY change_type"),
            )
            .await?,
        );
        if changed.inserted + changed.deleted == 0
            || changed.inserted + changed.deleted > INLINE_MAX_ROWS as u64
        {
            return Ok(changed);
        }
        let rows = self
            .run(
                workspace_id,
                &format!("SELECT cast(to_json(c) AS VARCHAR) FROM {from} c ORDER BY {SIDE_ORDER}"),
            )
            .await?;
        changed.rows = Some(split_sides(rows)?);
        Ok(changed)
    }

    async fn change_page(
        &self,
        workspace_id: Uuid,
        schema: &str,
        table: &str,
        snapshot: i64,
        limit: usize,
        offset: usize,
    ) -> Result<ChangePage, ReadError> {
        let from = changes_fn(schema, table, snapshot);
        // Each side's size, so each pages from its own offset within the one
        // ordering: the inserted side is rows `[0, inserted)`, the deleted side
        // the rest.
        let sides = tally(
            self.run(
                workspace_id,
                &format!("SELECT change_type, count(*) FROM {from} GROUP BY change_type"),
            )
            .await?,
        );
        let (ins, del) = (sides.inserted as usize, sides.deleted as usize);
        let take = |len: usize| len.saturating_sub(offset).min(limit.saturating_add(1));
        let page = |start: usize, n: usize| {
            format!(
                "SELECT cast(to_json(c) AS VARCHAR) FROM {from} c \
                 ORDER BY {SIDE_ORDER} LIMIT {n} OFFSET {start}"
            )
        };
        let mut inserted = match take(ins) {
            0 => Vec::new(),
            n => split_sides(self.run(workspace_id, &page(offset, n)).await?)?.0,
        };
        let mut deleted = match take(del) {
            0 => Vec::new(),
            n => {
                split_sides(
                    self.run(workspace_id, &page(ins.saturating_add(offset), n))
                        .await?,
                )?
                .1
            }
        };
        let has_more = inserted.len() > limit || deleted.len() > limit;
        inserted.truncate(limit);
        deleted.truncate(limit);
        Ok(ChangePage {
            inserted,
            deleted,
            has_more,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_change_is_an_insert_or_a_delete_of_the_table() {
        for made in [
            "inserted_into_table:3",
            "deleted_from_table:3",
            "inlined_insert:3",
            "inlined_delete:3",
            "inlined_insert:2,inlined_delete:3",
            "created_table:\"bk\".\"big\",inserted_into_table:3",
        ] {
            assert!(changes_rows_of(made, 3), "{made}");
        }
    }

    #[test]
    fn compaction_and_other_tables_are_not_changes() {
        for made in [
            "merge_adjacent:3",
            "rewrite_delete:3",
            // The raw kind; `ducklake_snapshots().changes` calls it
            // `flushed_inlined`. Airhouse pins it.
            "inline_flush:3",
            "inserted_into_table:2",
            "inlined_insert:30",
            "created_table:\"s\".\"x,inlined_insert:3\"",
            "altered_table:3",
        ] {
            assert!(!changes_rows_of(made, 3), "{made}");
        }
    }

    #[test]
    fn a_removed_renamed_or_retyped_column_breaks_the_contract() {
        let contract: Columns = [("id", "INTEGER"), ("amt", "INTEGER"), ("note", "VARCHAR")]
            .into_iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
        let mut now = contract.clone();
        assert!(broken_columns(&contract, &now).is_empty());
        now.insert("added".into(), "INTEGER".into());
        assert!(broken_columns(&contract, &now).is_empty(), "adding is fine");
        now.remove("note");
        now.insert("memo".into(), "VARCHAR".into());
        assert_eq!(
            broken_columns(&contract, &now).len(),
            1,
            "a rename is a removal"
        );
        now.insert("amt".into(), "VARCHAR".into());
        let broken = broken_columns(&contract, &now);
        assert_eq!(broken.len(), 2);
        assert!(
            broken.iter().any(|b| b.contains("`amt` changed type")),
            "{broken:?}"
        );
        now.insert("rowid".into(), "BIGINT".into());
        assert!(
            broken_columns(&contract, &now)
                .iter()
                .any(|b| b.contains("clashes"))
        );
    }

    #[test]
    fn the_changes_call_names_the_schema_and_a_bare_table() {
        assert_eq!(
            changes_fn("bk", "lines", 7),
            "ducklake_table_changes('lake', 'bk', 'lines', 7, 7)"
        );
        assert_eq!(literal("it's"), "'it''s'");
    }

    #[test]
    fn an_airhouse_error_is_classified() {
        assert!(matches!(
            classify("Invalid Input Error: No snapshot found at version 3".into()),
            ReadError::SnapshotGone(_)
        ));
        assert!(matches!(
            classify("Catalog Error: Table with name x does not exist".into()),
            ReadError::TableMissing(_)
        ));
        assert!(matches!(classify("boom".into()), ReadError::Failed(_)));
    }

    #[test]
    fn a_changed_row_loses_the_columns_table_changes_added() {
        let (change_type, row) =
            parse_change(r#"{"snapshot_id":3,"rowid":0,"change_type":"insert","id":1,"note":"a"}"#)
                .unwrap();
        assert_eq!(change_type, "insert");
        assert_eq!(row.len(), 2);
        assert!(row.contains_key("id") && row.contains_key("note"));
    }

    fn cell(text: &str) -> Vec<Option<String>> {
        vec![Some(text.to_string())]
    }

    /// The sides are split by each row's `change_type`, since the SQL may not
    /// filter on it: an update's post-image is inserted, its pre-image deleted.
    #[test]
    fn changed_rows_are_split_by_their_change_type() {
        let (inserted, deleted) = split_sides(vec![
            cell(r#"{"rowid":0,"change_type":"insert","id":1}"#),
            cell(r#"{"rowid":1,"change_type":"update_postimage","id":2,"v":"B"}"#),
            cell(r#"{"rowid":1,"change_type":"update_preimage","id":2,"v":"b"}"#),
            cell(r#"{"rowid":2,"change_type":"delete","id":3}"#),
        ])
        .unwrap();
        let ids = |rows: &[Row]| rows.iter().map(|r| r["id"].clone()).collect::<Vec<_>>();
        assert_eq!(ids(&inserted), [1, 2]);
        assert_eq!(ids(&deleted), [2, 3]);
        assert_eq!(deleted[0]["v"], "b");
        assert!(split_sides(vec![cell(r#"{"change_type":"moved"}"#)]).is_err());
        assert!(split_sides(vec![cell(r#"{"id":1}"#)]).is_err());
    }

    /// The counts by `change_type`: an update is on both sides and counted once
    /// as an update.
    #[test]
    fn a_tally_counts_an_update_on_both_sides() {
        let row = |t: &str, n: &str| vec![Some(t.to_string()), Some(n.to_string())];
        let changed = tally(vec![
            row("insert", "3"),
            row("update_postimage", "2"),
            row("update_preimage", "2"),
            row("delete", "1"),
        ]);
        assert_eq!(
            (changed.inserted, changed.deleted, changed.updated),
            (5, 3, 2)
        );
    }
}
