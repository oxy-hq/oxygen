//! The tenant side of one relay batch, as SQL: what the relay reads from a
//! store's change log, how it numbers a batch before shipping and confirms it
//! after, and how it deletes only what it shipped.
//!
//! Every statement runs as the store's owner, the only role that reads
//! `oxy_changes`. Nothing here opens a connection.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// The stream's one row: `generation, capturing, restamping`, or no row where
/// capture was never installed.
pub const STREAM_SQL: &str = "SELECT generation, capturing, restamping FROM oxy_changes.stream";

/// `$1 bigint`: how many log rows are older than that generation.
pub const OLDER_GENERATION_SQL: &str =
    "SELECT count(*)::bigint FROM oxy_changes.log WHERE generation < $1::bigint";

/// `$1 generation`: one row per writer with changes waiting, as `writer`,
/// `rows bigint` and `oldest timestamptz`.
pub const WAITING_SQL: &str = "SELECT writer, count(*)::bigint AS rows, min(changed_at) AS oldest \
     FROM oxy_changes.log WHERE generation = $1::bigint GROUP BY writer";

/// `$1 writer`: its `intent_batch, shipped_batch`, or no row.
pub const PROGRESS_SQL: &str = "SELECT intent_batch, shipped_batch FROM oxy_changes.progress \
     WHERE writer = $1::text";

/// `$1 writer`: every recorded shape of that writer's tables, as `table_name,
/// relid::bigint, schema_version, columns::text, primary_key, excluded`.
pub const SHAPES_SQL: &str = "SELECT t.table_name, t.relid::bigint AS relid, s.schema_version, \
     s.columns::text AS columns, s.primary_key, s.excluded \
     FROM oxy_changes.tables t JOIN oxy_changes.shapes s ON s.relid = t.relid \
     WHERE t.writer = $1::text ORDER BY t.table_name, s.schema_version";

/// `$1 generation bigint, $2 writer text, $3 held text[], $4 limit bigint,
/// $5 cap bigint`.
///
/// The first `$4` committed rows by `seq`, plus every other row of each of
/// their transactions, ordered by `seq`, so a batch ends where a source
/// transaction ends and one larger than `$4` ships whole. The rows come from one
/// `txid IN (head)` test, which every head row satisfies by its own txid and
/// which `log_writer_txid` serves. Tables in `$3` are
/// left out of both halves. A transaction still open is invisible here, and
/// its rows will carry a `seq` below ones this read returns: that is why
/// [`DELETE_SHIPPED_SQL`] names exact `seq`s.
///
/// At most `$5` rows come back. The caller passes its ceiling plus one, and
/// ships nothing of a read that reaches it: a capped read may end inside a
/// transaction, so it is only ever counted, never shipped.
pub const READ_BATCH_SQL: &str = "WITH eligible AS ( \
       SELECT seq, txid FROM oxy_changes.log \
       WHERE generation = $1::bigint AND writer = $2::text \
         AND NOT (table_name = ANY($3::text[])) ), \
     head AS (SELECT txid FROM eligible ORDER BY seq LIMIT $4::bigint) \
     SELECT l.seq, l.txid::text AS txid, l.table_name, l.schema_version, \
            l.op::text AS op, l.row_image::text AS row_image, l.actor, l.invocation, \
            l.provenance, l.changed_at \
     FROM oxy_changes.log l \
     WHERE l.generation = $1::bigint AND l.writer = $2::text \
       AND NOT (l.table_name = ANY($3::text[])) \
       AND l.txid IN (SELECT txid FROM head) \
     ORDER BY l.seq LIMIT $5::bigint";

/// `$1 generation bigint, $2 writer text, $3 held text[], $4 txid text`: how
/// many rows of that source transaction the writer's batch would read, its
/// held tables left out, as one `bigint`. For a transaction too large to
/// ship, which [`READ_BATCH_SQL`]'s cap stopped reading; `log_writer_txid`
/// serves it.
pub const TRANSACTION_ROWS_SQL: &str = "SELECT count(*)::bigint FROM oxy_changes.log \
     WHERE generation = $1::bigint AND writer = $2::text AND txid = $4::text::xid8 \
       AND NOT (table_name = ANY($3::text[]))";

/// `$1 writer`: upsert `intent_batch` to the next batch number and return it.
/// Numbers only grow across writers, because they share one sequence.
pub const DECLARE_INTENT_SQL: &str = "INSERT INTO oxy_changes.progress (writer, intent_batch) \
     VALUES ($1::text, nextval('oxy_changes.batches')) \
     ON CONFLICT (writer) DO UPDATE SET intent_batch = EXCLUDED.intent_batch \
     RETURNING intent_batch";

/// `$1 writer, $2 batch`: record the batch shipped, only where it is the
/// intent declared. One row affected, or none.
pub const CONFIRM_SQL: &str = "UPDATE oxy_changes.progress SET shipped_batch = $2::bigint \
     WHERE writer = $1::text AND intent_batch = $2::bigint";

/// `$1 generation, $2 bigint[]`: delete exactly those `seq`s. Never
/// `seq <= max`: a transaction that committed after the batch was read can
/// hold a lower `seq` than the batch's highest.
pub const DELETE_SHIPPED_SQL: &str = "DELETE FROM oxy_changes.log \
     WHERE generation = $1::bigint AND seq = ANY($2::bigint[])";

/// The stream's one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamState {
    pub generation: i64,
    pub capturing: bool,
    /// The generation a bump is re-stamping from, while one runs.
    pub restamping: Option<i64>,
}

impl StreamState {
    /// A row of [`STREAM_SQL`].
    pub fn from_row(row: &tokio_postgres::Row) -> Result<Self, tokio_postgres::Error> {
        Ok(Self {
            generation: row.try_get("generation")?,
            capturing: row.try_get("capturing")?,
            restamping: row.try_get("restamping")?,
        })
    }
}

/// One row of [`READ_BATCH_SQL`].
#[derive(Debug, Clone, PartialEq)]
pub struct LogRow {
    pub seq: i64,
    pub txid: u64,
    pub table_name: String,
    pub schema_version: i32,
    /// `i`, `u`, `d` or `t`.
    pub op: char,
    /// The image as `row_image::text`, unparsed: the relay reads each value's
    /// JSON lexeme, so a `numeric` keeps every digit (no f64 on the way).
    pub row_image: Option<String>,
    pub actor: Option<String>,
    pub invocation: Option<Uuid>,
    pub provenance: Option<Vec<u8>>,
    pub changed_at: DateTime<Utc>,
}

impl LogRow {
    /// A row of [`READ_BATCH_SQL`]; `Err` names what did not decode.
    pub fn from_row(row: &tokio_postgres::Row) -> Result<Self, String> {
        let text = |e: tokio_postgres::Error| e.to_string();
        let txid: String = row.try_get("txid").map_err(text)?;
        let op: String = row.try_get("op").map_err(text)?;
        Ok(Self {
            seq: row.try_get("seq").map_err(text)?,
            txid: txid.parse().map_err(|e| format!("txid {txid:?}: {e}"))?,
            table_name: row.try_get("table_name").map_err(text)?,
            schema_version: row.try_get("schema_version").map_err(text)?,
            op: op.chars().next().ok_or_else(|| "an empty op".to_string())?,
            row_image: row.try_get("row_image").map_err(text)?,
            actor: row.try_get("actor").map_err(text)?,
            invocation: row.try_get("invocation").map_err(text)?,
            provenance: row.try_get("provenance").map_err(text)?,
            changed_at: row.try_get("changed_at").map_err(text)?,
        })
    }
}
