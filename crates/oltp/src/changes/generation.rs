//! The tenant side of a generation bump, as SQL (OLTP change stream design
//! §5.6). A bump moves the stream to a new generation and re-stamps the log's
//! rows of the old one in chunks afterwards, so the bump itself is one short
//! owner transaction that does not grow with the backlog:
//!
//! 1. [`bump_lock_sql`]: wait out every open capturing transaction, or fail
//!    with `55P03` and change nothing;
//! 2. [`BUMP_STREAM_SQL`], which must return exactly one row;
//! 3. [`BUMP_ABANDON_SQL`], [`BUMP_PROGRESS_SQL`], [`BUMP_REQUEST_SQL`];
//! 4. `COMMIT`.
//!
//! The re-stamp then runs [`RESTAMP_CHUNK_SQL`] until it moves no row, and
//! [`END_RESTAMP_SQL`] ends it. A row of an older generation found later (a
//! transaction that spanned the bump) is re-stamped by
//! [`RESTAMP_OLDER_CHUNK_SQL`]. Every statement is schema-qualified, as
//! [`pin_session_sql`](super::pin_session_sql) leaves only `pg_catalog` on the
//! search path.

use std::time::Duration;

use super::lock_timeout_sql;

/// `SET LOCAL lock_timeout = …`, then `LOCK TABLE oxy_changes.stream IN ACCESS
/// EXCLUSIVE MODE`: capture reads the stream row, so the lock waits for every
/// open transaction that wrote a captured table, and holds off new ones.
pub fn bump_lock_sql(lock_timeout: Duration) -> [String; 2] {
    [
        lock_timeout_sql(lock_timeout),
        "LOCK TABLE oxy_changes.stream IN ACCESS EXCLUSIVE MODE".to_string(),
    ]
}

/// `$1` the new generation: the old one, and `restamping` set to it. No row
/// while a re-stamp is running, or unless `$1` is greater.
pub const BUMP_STREAM_SQL: &str = "UPDATE oxy_changes.stream \
     SET restamping = generation, generation = $1 \
     WHERE restamping IS NULL AND generation < $1 RETURNING restamping";

/// Abandon every requested or copying baseline, so none of the old generation
/// holds the `baselines_requested` index against the new one's request.
pub const BUMP_ABANDON_SQL: &str = "UPDATE oxy_changes.baselines SET state = 'abandoned' \
     WHERE state IN ('requested', 'copying')";

/// Every writer's relay starts its batches again.
pub const BUMP_PROGRESS_SQL: &str =
    "UPDATE oxy_changes.progress SET intent_batch = 0, shipped_batch = 0";

/// `$1` the new generation: one `bump` request in it per installed table.
pub const BUMP_REQUEST_SQL: &str = "INSERT INTO oxy_changes.baselines \
     (relid, writer, table_name, generation, reason, state) \
     SELECT t.relid, t.writer, t.table_name, $1::bigint, 'bump', 'requested' \
       FROM oxy_changes.tables t WHERE t.installed \
     ON CONFLICT (relid) WHERE state = 'requested' \
     DO UPDATE SET attempts = 0, retry_at = NULL";

/// `$1` from, `$2` to, `$3` limit: move at most `$3` log rows, lowest `seq`
/// first, from generation `$1` to `$2`, each keeping its `seq`. Returns the
/// rows moved; 0 means the re-stamp is done.
pub const RESTAMP_CHUNK_SQL: &str = "UPDATE oxy_changes.log SET generation = $2 \
     WHERE generation = $1 AND seq IN \
       (SELECT seq FROM oxy_changes.log WHERE generation = $1 ORDER BY seq LIMIT $3)";

/// `$1` to, `$2` limit: move at most `$2` log rows of any generation older
/// than `$1`, lowest `seq` first, to `$1`, each keeping its `seq`: the rows
/// of a transaction that spanned a bump. Returns the rows moved; 0 means
/// none is left.
pub const RESTAMP_OLDER_CHUNK_SQL: &str = "UPDATE oxy_changes.log SET generation = $1 \
     WHERE generation < $1 AND seq IN \
       (SELECT seq FROM oxy_changes.log WHERE generation < $1 ORDER BY seq LIMIT $2)";

/// `$1` from: end the re-stamp of generation `$1`.
pub const END_RESTAMP_SQL: &str =
    "UPDATE oxy_changes.stream SET restamping = NULL WHERE restamping = $1";
