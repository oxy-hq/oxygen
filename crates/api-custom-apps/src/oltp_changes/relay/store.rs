//! The relay's side of the org's store: its owner connection, and the reads
//! and writes of one pass (`oxy_oltp::changes::relay`), each failure said as
//! what was being done and the server's own message.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use oxy_oltp::changes::{
    CONFIRM_SQL, Column, DECLARE_INTENT_SQL, DELETE_SHIPPED_SQL, LogRow, OLDER_GENERATION_SQL,
    PENDING_VIEWS_SQL, PROGRESS_SQL, READ_BATCH_SQL, SHAPES_SQL, STREAM_SQL, StreamState,
    TRANSACTION_ROWS_SQL, VIEW_PENDING_SQL, VIEW_RENDERED_SQL, WAITING_SQL, pin_session_sql,
};
use sea_orm::DatabaseConnection;
use tokio_postgres::types::ToSql;
use tokio_postgres::{Client, Row, Transaction};
use uuid::Uuid;

use super::table::ShapeVersion;
use super::{BASELINE_STATEMENT_TIMEOUT, BEGIN_LOCK_TIMEOUT, Limits};
use crate::oltp_changes::owner::{current_tenant, owner_client};

/// A writer with changes waiting in the current generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Waiting {
    pub writer: String,
    pub rows: i64,
    pub oldest: DateTime<Utc>,
}

/// A batch read and declared: its number, and its rows in `seq` order.
pub(super) struct Read {
    pub batch: i64,
    pub rows: Vec<LogRow>,
    /// Whether more may wait behind it: it read its whole limit, or was cut
    /// back under the ceiling.
    pub full: bool,
}

/// What a writer's next read found.
pub(super) enum Next {
    /// Nothing waits outside the held tables; nothing was declared.
    Empty,
    /// A batch, read and declared.
    Read(Read),
    /// The oldest waiting source transaction alone has `rows` rows, past the
    /// relay's ceiling; nothing was declared.
    Over { rows: i64 },
}

/// A failed statement as `doing` and the server's own message.
pub(super) fn pg(doing: &str) -> impl Fn(tokio_postgres::Error) -> String + '_ {
    move |e| format!("{doing}: {}", oxy_oltp::connect::pg_detail(&e))
}

/// The org's store, open as its owner.
pub(super) struct Opened {
    pub owner: Client,
    /// The store's database, which alarms name.
    pub database: String,
}

/// The org's store as its owner, brought current. `None` when the org has
/// none, it is not active, or it never reached the step that creates
/// `oxy_changes`: nothing was ever captured there.
pub(super) async fn open(db: &DatabaseConnection, org_id: Uuid) -> Result<Option<Opened>, String> {
    let Some(tenant) = current_tenant(db, org_id, false).await? else {
        return Ok(None);
    };
    let owner = owner_client(&tenant, "oxy_changes relay").await?;
    Ok(Some(Opened {
        owner,
        database: tenant.database_name,
    }))
}

/// The stream's row; `None` where capture was never installed.
pub(super) async fn stream(owner: &Client) -> Result<Option<StreamState>, String> {
    let doing = "read the change stream's state";
    let row = owner.query_opt(STREAM_SQL, &[]).await.map_err(pg(doing))?;
    row.as_ref()
        .map(StreamState::from_row)
        .transpose()
        .map_err(pg(doing))
}

/// How many log rows are older than `generation`.
pub(super) async fn older_generation_rows(owner: &Client, generation: i64) -> Result<i64, String> {
    let doing = "count the change log's older-generation rows";
    let row = owner
        .query_one(OLDER_GENERATION_SQL, &[&generation])
        .await
        .map_err(pg(doing))?;
    row.try_get(0).map_err(pg(doing))
}

/// Every writer with changes waiting in `generation`, the longest waiting
/// first.
pub(super) async fn waiting(owner: &Client, generation: i64) -> Result<Vec<Waiting>, String> {
    let doing = "read the writers with changes waiting";
    let rows = owner
        .query(WAITING_SQL, &[&generation])
        .await
        .map_err(pg(doing))?;
    let mut waiting = rows
        .iter()
        .map(|row| -> Result<Waiting, tokio_postgres::Error> {
            Ok(Waiting {
                writer: row.try_get("writer")?,
                rows: row.try_get("rows")?,
                oldest: row.try_get("oldest")?,
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(pg(doing))?;
    waiting.sort_by(|a, b| {
        a.oldest
            .cmp(&b.oldest)
            .then_with(|| a.writer.cmp(&b.writer))
    });
    Ok(waiting)
}

/// `writer`'s `(intent_batch, shipped_batch)`, both 0 before its first batch.
pub(super) async fn progress(owner: &Client, writer: &str) -> Result<(i64, i64), String> {
    let doing = "read the writer's relay progress";
    let Some(row) = owner
        .query_opt(PROGRESS_SQL, &[&writer])
        .await
        .map_err(pg(doing))?
    else {
        return Ok((0, 0));
    };
    let read = |row: &tokio_postgres::Row| -> Result<(i64, i64), tokio_postgres::Error> {
        Ok((row.try_get("intent_batch")?, row.try_get("shipped_batch")?))
    };
    read(&row).map_err(pg(doing))
}

/// Every recorded shape of `writer`'s tables, by table, oldest
/// `schema_version` first. A table no longer in `oxy_changes.tables` has none.
pub(super) async fn shapes(
    owner: &Client,
    writer: &str,
) -> Result<HashMap<String, Vec<ShapeVersion>>, String> {
    let doing = "read the writer's recorded shapes";
    let rows = owner
        .query(SHAPES_SQL, &[&writer])
        .await
        .map_err(pg(doing))?;
    let mut shapes: HashMap<String, Vec<ShapeVersion>> = HashMap::new();
    for row in &rows {
        let (table, schema_version, columns, excluded, primary_key) =
            shape_row(row).map_err(pg(doing))?;
        let captured = captured(&columns, &excluded)
            .map_err(|e| format!("{doing}: {table:?} v{schema_version}: {e}"))?;
        shapes.entry(table).or_default().push(ShapeVersion {
            schema_version,
            captured,
            primary_key,
        });
    }
    Ok(shapes)
}

/// A row of `SHAPES_SQL`: `table_name, schema_version, columns, excluded,
/// primary_key`.
fn shape_row(
    row: &tokio_postgres::Row,
) -> Result<(String, i32, String, Vec<String>, Vec<String>), tokio_postgres::Error> {
    Ok((
        row.try_get("table_name")?,
        row.try_get("schema_version")?,
        row.try_get("columns")?,
        row.try_get("excluded")?,
        row.try_get("primary_key")?,
    ))
}

/// A shape's captured columns: its `columns` (JSON text) without the
/// `excluded` ones.
fn captured(columns: &str, excluded: &[String]) -> Result<Vec<Column>, String> {
    let columns: Vec<Column> =
        serde_json::from_str(columns).map_err(|e| format!("columns: {e}"))?;
    Ok(columns
        .into_iter()
        .filter(|c| !excluded.contains(&c.name))
        .collect())
}

/// What to read: a writer's tables of a generation, but its held ones.
pub(super) struct Source<'a> {
    pub generation: i64,
    pub writer: &'a str,
    pub held: &'a [String],
}

/// In one transaction: read `source`'s next batch, and declare its number as
/// the writer's intent. A batch is whole source transactions, extended past
/// `limits.batch_rows` to the end of its last one, and never past
/// `limits.batch_ceiling` rows:
///
/// - several transactions that pass the ceiling together are cut back to the
///   oldest alone, a batch of its own ([`Read::full`], since more wait);
/// - one transaction that passes it alone is [`Next::Over`], with nothing
///   declared, so no intent is left behind.
pub(super) async fn read_and_declare(
    owner: &mut Client,
    source: &Source<'_>,
    limits: &Limits,
) -> Result<Next, String> {
    let doing = "read and declare the writer's next batch";
    let txn = owner.transaction().await.map_err(pg(doing))?;
    let (rows, cut_back) = read_capped(&txn, source, limits).await?;
    if rows.is_empty() || count(&rows) > limits.batch_ceiling {
        let next = match rows.first() {
            None => Next::Empty,
            Some(first) => Next::Over {
                rows: transaction_rows(&txn, source, first).await?,
            },
        };
        txn.rollback().await.map_err(pg(doing))?;
        return Ok(next);
    }
    let full = cut_back || count(&rows) >= limits.batch_rows;
    declare(txn, source.writer, &rows, full)
        .await
        .map(Next::Read)
}

fn count(rows: &[Row]) -> i64 {
    i64::try_from(rows.len()).unwrap_or(i64::MAX)
}

/// The batch's rows, and whether they were cut back to the oldest
/// transaction alone because the whole head passed the ceiling.
async fn read_capped(
    txn: &Transaction<'_>,
    source: &Source<'_>,
    limits: &Limits,
) -> Result<(Vec<Row>, bool), String> {
    let rows = read_rows(txn, source, limits.batch_rows, limits).await?;
    if count(&rows) <= limits.batch_ceiling || limits.batch_rows <= 1 {
        return Ok((rows, false));
    }
    Ok((read_rows(txn, source, 1, limits).await?, true))
}

/// Declare `rows` as `writer`'s next batch, and commit.
async fn declare(
    txn: Transaction<'_>,
    writer: &str,
    rows: &[Row],
    full: bool,
) -> Result<Read, String> {
    let doing = "declare the writer's next batch";
    let rows = rows
        .iter()
        .map(LogRow::from_row)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("{doing}: a log row: {e}"))?;
    let declared = txn
        .query_one(DECLARE_INTENT_SQL, &[&writer])
        .await
        .map_err(pg(doing))?;
    let batch: i64 = declared.try_get("intent_batch").map_err(pg(doing))?;
    txn.commit().await.map_err(pg(doing))?;
    Ok(Read { batch, rows, full })
}

/// `READ_BATCH_SQL` from a head of `head` rows, capped one row past the
/// ceiling: a read that reaches the cap is only ever counted.
async fn read_rows(
    txn: &Transaction<'_>,
    source: &Source<'_>,
    head: i64,
    limits: &Limits,
) -> Result<Vec<Row>, String> {
    let cap = limits.batch_ceiling.saturating_add(1);
    let params: [&(dyn ToSql + Sync); 5] = [
        &source.generation,
        &source.writer,
        &source.held,
        &head,
        &cap,
    ];
    txn.query(READ_BATCH_SQL, &params)
        .await
        .map_err(pg("read the writer's next batch"))
}

/// How many rows of `first`'s source transaction the batch would read.
async fn transaction_rows(
    txn: &Transaction<'_>,
    source: &Source<'_>,
    first: &Row,
) -> Result<i64, String> {
    let doing = "count a transaction past the relay's ceiling";
    let txid: String = first.try_get("txid").map_err(pg(doing))?;
    let row = txn
        .query_one(
            TRANSACTION_ROWS_SQL,
            &[&source.generation, &source.writer, &source.held, &txid],
        )
        .await
        .map_err(pg(doing))?;
    row.try_get(0).map_err(pg(doing))
}

/// A batch to confirm: its number and generation, the `seqs` it shipped,
/// and the tables whose current-state views it needs made.
pub(super) struct Confirming<'a> {
    pub batch: i64,
    pub generation: i64,
    pub seqs: &'a [i64],
    pub views: &'a [String],
}

/// In one transaction: record the batch shipped, delete exactly the `seqs`
/// it shipped, and flag its views pending ([`VIEW_PENDING_SQL`]), so a pass
/// that dies before making them leaves them for the next. `None`, with
/// nothing deleted, when the writer's intent is no longer the batch: the
/// store was rewound under the relay. Otherwise whether the flags failed:
/// they never stop the confirm.
pub(super) async fn confirm_and_delete(
    owner: &mut Client,
    writer: &str,
    batch: &Confirming<'_>,
) -> Result<Option<Result<(), String>>, String> {
    let doing = "confirm the batch and delete what it shipped";
    let mut txn = owner.transaction().await.map_err(pg(doing))?;
    let confirmed = txn
        .execute(CONFIRM_SQL, &[&writer, &batch.batch])
        .await
        .map_err(pg(doing))?;
    if confirmed != 1 {
        txn.rollback().await.map_err(pg(doing))?;
        return Ok(None);
    }
    txn.execute(DELETE_SHIPPED_SQL, &[&batch.generation, &batch.seqs])
        .await
        .map_err(pg(doing))?;
    let flagged = flag_views(&mut txn, writer, batch.views).await;
    txn.commit().await.map_err(pg(doing))?;
    Ok(Some(flagged))
}

/// Flag `views` pending inside `txn`, under a savepoint: a failure flags
/// none, and leaves the transaction to commit.
async fn flag_views(
    txn: &mut Transaction<'_>,
    writer: &str,
    views: &[String],
) -> Result<(), String> {
    if views.is_empty() {
        return Ok(());
    }
    let doing = "record the batch's views pending";
    let point = txn.savepoint("views").await.map_err(pg(doing))?;
    for table in views {
        if let Err(e) = point.execute(VIEW_PENDING_SQL, &[&writer, table]).await {
            point.rollback().await.map_err(pg(doing))?;
            return Err(pg(doing)(e));
        }
    }
    point.commit().await.map_err(pg(doing))
}

/// Every table whose current-state view waits to be made again
/// ([`PENDING_VIEWS_SQL`]), as `(writer, table)`.
pub(super) async fn pending_views(owner: &Client) -> Result<Vec<(String, String)>, String> {
    let doing = "read the views left pending";
    let rows = owner
        .query(PENDING_VIEWS_SQL, &[])
        .await
        .map_err(pg(doing))?;
    rows.iter()
        .map(|row| Ok((row.try_get("writer")?, row.try_get("table_name")?)))
        .collect::<Result<_, tokio_postgres::Error>>()
        .map_err(pg(doing))
}

/// Flag `writer`'s `table`'s current-state view as pending
/// ([`VIEW_PENDING_SQL`]).
pub(super) async fn view_pending(
    owner: &mut Client,
    writer: &str,
    table: &str,
) -> Result<(), String> {
    let doing = "record the current-state view pending";
    pinned_update(owner, doing, VIEW_PENDING_SQL, writer, table).await
}

/// Clear `writer`'s `table`'s pending view flag ([`VIEW_RENDERED_SQL`]).
pub(super) async fn view_rendered(
    owner: &mut Client,
    writer: &str,
    table: &str,
) -> Result<(), String> {
    let doing = "record the current-state view made";
    pinned_update(owner, doing, VIEW_RENDERED_SQL, writer, table).await
}

/// `sql` (`$1 writer, $2 table`) as the owner, one statement in a
/// transaction pinned as a baseline's steps are ([`pin_session_sql`]).
async fn pinned_update(
    owner: &mut Client,
    doing: &str,
    sql: &str,
    writer: &str,
    table: &str,
) -> Result<(), String> {
    let txn = owner.transaction().await.map_err(pg(doing))?;
    let pin = pin_session_sql(BEGIN_LOCK_TIMEOUT, BASELINE_STATEMENT_TIMEOUT);
    txn.batch_execute(&pin).await.map_err(pg(doing))?;
    txn.execute(sql, &[&writer, &table])
        .await
        .map_err(pg(doing))?;
    txn.commit().await.map_err(pg(doing))
}

#[cfg(test)]
mod tests;
