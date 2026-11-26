//! The tenant side of a baseline, as SQL (OLTP change stream design §5.5).
//!
//! A baseline restarts one declared table's current state in Airhouse from a
//! fresh copy of its rows, as a window in `oxy_changes.log`: a `b` marker
//! written under the table's `SHARE` lock, chunks of `s` rows copied under
//! `FOR SHARE` row locks, then a `c` marker naming the begin.
//! `oxy_changes.baselines` records each request and how far its copy has got;
//! at most one is `requested` and one `copying` per table.
//!
//! **Two sessions, and their order is the point.** The store's owner holds no
//! privilege on a writer's table and is a member of no writer role, so it can
//! take neither lock. The writer may not write the log, and no function it can
//! execute writes it, or an app could forge history. So for a begin and for
//! each chunk:
//! 1. the writer's session pins its search path and timeouts
//!    ([`pin_session_sql`]) and takes the lock: [`begin_lock_sql`], or the
//!    `FOR SHARE` of [`chunk_read_sql`];
//! 2. the owner's session writes the marker or the rows ([`LOCK_REQUEST_SQL`]
//!    then [`BEGIN_SQL`], or [`CHUNK_WRITE_SQL`]) and commits **first**, so
//!    they draw their `seq` while the table's writers are held off;
//! 3. only then does the writer's session commit, releasing the lock.
//!
//! Every later change to a locked row, or to a table past its begin, then has
//! a higher `seq` than the snapshot row or the marker.
//!
//! **A request and a begin take turns** on the table's open `baselines` rows:
//! a request locks them `FOR UPDATE` before it abandons or inserts anything,
//! and a begin locks its requested row ([`LOCK_REQUEST_SQL`]) before
//! [`BEGIN_SQL`], which then runs on a fresh snapshot. So a begin waiting on an
//! install's request sees that install's `installed = false` or new shape and
//! does not begin, and a request waiting on a begin sees, and abandons, the
//! baseline it began.
//!
//! A snapshot image is built by the expression a capture image is
//! ([`super::capture`]): the row as `jsonb`, filtered to the captured columns'
//! names, which reach the SQL only as literals. Only the primary key's columns
//! are named as identifiers, quoted, since a copy orders and resumes by them,
//! each with the type [`KEY_TYPES_SQL`] reads for it.
//!
//! Every statement but the two locks and [`KEY_TYPES_SQL`] runs as the owner.
//! Nothing here opens a connection.

use std::time::Duration;

use tokio_postgres::IsolationLevel;

use crate::platform::{CHANGES_BASELINES_TABLE, CHANGES_STREAM_TABLE, CHANGES_TABLES_TABLE};
use crate::roles::quote_literal;
use crate::schema::{SchemaError, WriterRef, quote_ident, validate_name};

use super::capture::{filtered, text_array};

/// The actor of every snapshot row, signed over the copying transaction's
/// txid. [`CHUNK_WRITE_SQL`] writes it.
pub const PLATFORM_ACTOR: &str = "platform";

/// Why a baseline was requested, as `baselines.reason` records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaselineReason {
    /// The table was declared, or declared again.
    OptIn,
    /// An install recorded a new shape.
    Reshape,
    /// The relay repaired drifted capture.
    Drift,
    /// An operator resynced the table.
    Resync,
    /// A generation bump.
    Bump,
    /// An installed table had no baseline in the stream's generation.
    Missing,
}

impl BaselineReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OptIn => "opt_in",
            Self::Reshape => "reshape",
            Self::Drift => "drift",
            Self::Resync => "resync",
            Self::Bump => "bump",
            Self::Missing => "missing",
        }
    }
}

/// Lock the table's open baselines, abandon its copying one, then request one
/// unless one is requested. Run all three in one transaction, in order: the
/// lock makes a begin of the requested row wait for this transaction (module
/// docs). The insert locks a requested row it finds, as an update, so a
/// request that raced another (`REQUEST_MISSING_SQL`, say) still holds the
/// row a begin must wait for; the update clears any backoff, so a promote's
/// request and nudge retry the baseline at once. It has no `installed` filter: an
/// install writes the request while `installed` is false, and the begin is
/// gated on it.
pub fn request_baseline_sql(relid: u32, reason: BaselineReason) -> [String; 3] {
    request(
        &format!("relid = {relid}::oid"),
        &format!("t.relid = {relid}::oid"),
        reason,
    )
}

/// [`request_baseline_sql`], for `writer`'s table named `table`, both
/// validated: for a caller that knows a table by name, not by oid.
pub fn request_named_baseline_sql(
    writer: &str,
    table: &str,
    reason: BaselineReason,
) -> Result<[String; 3], SchemaError> {
    validate_name(writer)?;
    validate_name(table)?;
    let named = format!(
        "t.writer = {} AND t.table_name = {}",
        quote_literal(writer),
        quote_literal(table)
    );
    Ok(request(
        &format!("relid IN (SELECT t.relid FROM {CHANGES_TABLES_TABLE} t WHERE {named})"),
        &named,
        reason,
    ))
}

/// The three statements of a request: `baseline` picks the table's
/// `baselines` rows, `table` its `tables t` row.
fn request(baseline: &str, table: &str, reason: BaselineReason) -> [String; 3] {
    [
        format!(
            "SELECT 1 FROM {CHANGES_BASELINES_TABLE} \
             WHERE {baseline} AND state IN ('requested', 'copying') FOR UPDATE"
        ),
        format!(
            "UPDATE {CHANGES_BASELINES_TABLE} SET state = 'abandoned' \
             WHERE {baseline} AND state = 'copying'"
        ),
        format!(
            "INSERT INTO {CHANGES_BASELINES_TABLE} \
             (relid, writer, table_name, generation, reason, state) \
             SELECT t.relid, t.writer, t.table_name, s.generation, '{reason}', 'requested' \
             FROM {CHANGES_TABLES_TABLE} t, {CHANGES_STREAM_TABLE} s WHERE {table} \
             ON CONFLICT (relid) WHERE state = 'requested' \
             DO UPDATE SET requested_at = {CHANGES_BASELINES_TABLE}.requested_at, \
                           attempts = 0, retry_at = NULL",
            reason = reason.as_str()
        ),
    ]
}

/// Abandon the table's open baselines, requested or copying: for a table
/// whose capture is being removed.
pub fn abandon_baselines_sql(relid: u32) -> String {
    format!(
        "UPDATE {CHANGES_BASELINES_TABLE} SET state = 'abandoned' \
         WHERE relid = {relid}::oid AND state IN ('requested', 'copying')"
    )
}

/// Abandon every open baseline whose `tables` row is gone.
pub const ABANDON_ORPHANS_SQL: &str = "UPDATE oxy_changes.baselines b SET state = 'abandoned' \
     WHERE b.state IN ('requested', 'copying') \
       AND NOT EXISTS (SELECT 1 FROM oxy_changes.tables t WHERE t.relid = b.relid)";

/// Request a baseline of every installed table that has none in the stream's
/// generation, unless the stream is not capturing or is re-stamping. One row,
/// `writer, table_name`, per request made.
pub const REQUEST_MISSING_SQL: &str = "INSERT INTO oxy_changes.baselines \
     (relid, writer, table_name, generation, reason, state) \
     SELECT t.relid, t.writer, t.table_name, s.generation, 'missing', 'requested' \
       FROM oxy_changes.tables t, oxy_changes.stream s \
      WHERE t.installed AND s.capturing AND s.restamping IS NULL \
        AND NOT EXISTS (SELECT 1 FROM oxy_changes.baselines b WHERE b.relid = t.relid \
                          AND b.generation = s.generation AND b.state <> 'abandoned') \
     ON CONFLICT DO NOTHING RETURNING writer, table_name";

/// Every requested or copying baseline, in `id` order: `id`,
/// `relid::bigint`, `writer`, `table_name`, `state`, `schema_version`,
/// `begin_seq`, `last_key::text`, `copied`, `attempts`, `retry_at`,
/// `backing_off`: whether `retry_at` is still ahead, by the store's clock
/// ([`BACKOFF_SQL`]), and `retry_in`: the seconds until it, as `float8`, so a
/// caller on another clock reads a delay, never an instant.
pub const OPEN_BASELINES_SQL: &str = "SELECT id, relid::bigint AS relid, writer, table_name, \
       state, schema_version, begin_seq, last_key::text AS last_key, copied, attempts, retry_at, \
       coalesce(retry_at > now(), false) AS backing_off, \
       extract(epoch FROM retry_at - now())::float8 AS retry_in \
     FROM oxy_changes.baselines WHERE state IN ('requested', 'copying') ORDER BY id";

/// `$1 id`: back a baseline off after a begin or a chunk could not take its
/// locks, or its rows kept changing under the copy: `attempts` counts it, and
/// `retry_at` is `5 s · 2^attempts` (the count before this one) from now, at
/// most 30 minutes. One statement, in a transaction of its own. A begin or a
/// chunk that commits resets both ([`BEGIN_SQL`], [`CHUNK_WRITE_SQL`]).
/// Returns `attempts`, `retry_at`, and `delay`, the seconds until `retry_at`
/// as `float8`; no row when the baseline is no longer open.
pub const BACKOFF_SQL: &str = "UPDATE oxy_changes.baselines \
       SET attempts = attempts + 1, \
           retry_at = now() + least(interval '5 seconds' * power(2, least(attempts, 20)), \
                                    interval '30 minutes') \
     WHERE id = $1 AND state IN ('requested', 'copying') \
     RETURNING attempts, retry_at, extract(epoch FROM retry_at - now())::float8 AS delay";

/// `$1 writer, $2 table`: the table's current-state view could not be made
/// again; flag its latest complete baseline, so later passes render it
/// again until one succeeds ([`PENDING_VIEWS_SQL`], [`VIEW_RENDERED_SQL`]).
/// How many rows it flagged: none before the table's first complete baseline.
pub const VIEW_PENDING_SQL: &str = "UPDATE oxy_changes.baselines SET view_pending = true \
     WHERE id = (SELECT id FROM oxy_changes.baselines \
                  WHERE writer = $1 AND table_name = $2 AND state = 'complete' \
                  ORDER BY id DESC LIMIT 1)";

/// Every table whose current-state view waits to be made again, as `writer,
/// table_name`, by writer then table.
pub const PENDING_VIEWS_SQL: &str = "SELECT DISTINCT writer, table_name \
     FROM oxy_changes.baselines WHERE view_pending ORDER BY writer, table_name";

/// `$1 writer, $2 table`: the table's current-state view was made: clear its
/// flag ([`VIEW_PENDING_SQL`]).
pub const VIEW_RENDERED_SQL: &str = "UPDATE oxy_changes.baselines SET view_pending = false \
     WHERE writer = $1 AND table_name = $2 AND view_pending";

/// `$1 generation bigint, $2 writer text, $3 held text[]` (null: none), `$4
/// cap bigint`: how many log rows the writer's batches have still to ship,
/// its held tables left out, counted only up to the cap, as one `bigint`. A
/// copy is paced against it, so it never needs to count past its limit.
pub const BACKLOG_SQL: &str = "SELECT count(*)::bigint FROM (SELECT 1 FROM oxy_changes.log \
     WHERE generation = $1::bigint AND writer = $2::text \
       AND NOT (table_name = ANY(coalesce($3::text[], '{}'))) \
     LIMIT $4::bigint) x";

/// `$1 id`: lock a requested baseline's row, as the owner's first statement
/// of a begin, before [`BEGIN_SQL`] runs as a statement of its own. It waits
/// for a request of the same table in flight ([`request_baseline_sql`] locks
/// the row too), so [`BEGIN_SQL`]'s fresh snapshot sees what that request's
/// transaction committed. Returns `id`; no row means it is not requested.
pub const LOCK_REQUEST_SQL: &str =
    "SELECT id FROM oxy_changes.baselines WHERE id = $1 AND state = 'requested' FOR UPDATE";

/// `$1 id`: begin a requested baseline, writing its `b` marker, while the
/// writer's session holds the table's `SHARE` lock ([`begin_lock_sql`]). Run
/// it after [`LOCK_REQUEST_SQL`], in the same owner transaction but as a new
/// statement, so it reads a snapshot taken after that lock. Returns
/// `begin_seq, schema_version`. No row means not now: the baseline is not
/// requested, its table is not installed or not at its latest shape (an
/// install is in progress), another baseline of it is still copying, or the
/// stream is re-stamping. A begin clears any backoff.
pub const BEGIN_SQL: &str = "WITH s AS (SELECT generation FROM oxy_changes.stream \
     WHERE restamping IS NULL), \
     req AS (SELECT b.id, t.writer, t.table_name, t.schema_version \
               FROM oxy_changes.baselines b JOIN oxy_changes.tables t ON t.relid = b.relid \
               JOIN s ON s.generation = b.generation \
              WHERE b.id = $1 AND b.state = 'requested' AND t.installed \
                AND t.schema_version = (SELECT max(sh.schema_version) FROM oxy_changes.shapes sh \
                                         WHERE sh.relid = b.relid) \
                AND NOT EXISTS (SELECT 1 FROM oxy_changes.baselines c \
                                 WHERE c.relid = b.relid AND c.state = 'copying') \
                FOR UPDATE OF b), \
     marker AS (INSERT INTO oxy_changes.log (generation, writer, table_name, schema_version, op) \
                SELECT s.generation, req.writer, req.table_name, req.schema_version, 'b'::\"char\" \
                  FROM req, s RETURNING seq, schema_version) \
     UPDATE oxy_changes.baselines b SET state = 'copying', begin_seq = marker.seq, \
            schema_version = marker.schema_version, begun_at = now(), \
            attempts = 0, retry_at = NULL \
       FROM marker WHERE b.id = $1 \
     RETURNING b.begin_seq, b.schema_version";

/// `$1 id, $2 last_key, $3 keys text[], $4 images text[], $5 provenance
/// bytea`: write one chunk's `s` rows and move the baseline's cursor, in one
/// statement, while the writer's session holds the chunk's row locks
/// ([`chunk_read_sql`]). Returns `live` (0 when the baseline is no longer
/// copying, and nothing was written) and `written`, both `bigint`. `$2` is
/// the chunk's last key as `jsonb` text, bound as text so no number in it
/// passes through a float. A chunk clears any backoff.
pub const CHUNK_WRITE_SQL: &str = "WITH live AS (UPDATE oxy_changes.baselines \
       SET last_key = $2::text::jsonb, copied = copied + cardinality($3::text[]), \
           attempts = 0, retry_at = NULL \
     WHERE id = $1 AND state = 'copying' \
     RETURNING generation, writer, table_name, schema_version), \
     written AS (INSERT INTO oxy_changes.log (generation, writer, table_name, schema_version, op, \
                                              key, row_image, actor, provenance) \
                 SELECT live.generation, live.writer, live.table_name, live.schema_version, \
                        's'::\"char\", r.key::jsonb, r.image::jsonb, 'platform', $5 \
                   FROM live, unnest($3::text[], $4::text[]) AS r(key, image) \
                 RETURNING 1) \
     SELECT (SELECT count(*) FROM live) AS live, (SELECT count(*) FROM written) AS written";

/// `$1 id`: complete a copying baseline, writing its `c` marker, whose image
/// is `{"begin_seq": …}`. Returns the marker's `seq`; no row means the
/// baseline was abandoned.
pub const COMPLETE_SQL: &str = "WITH done AS (UPDATE oxy_changes.baselines \
       SET state = 'complete', completed_at = now() \
     WHERE id = $1 AND state = 'copying' \
     RETURNING generation, writer, table_name, schema_version, begin_seq), \
     marker AS (INSERT INTO oxy_changes.log (generation, writer, table_name, schema_version, op, \
                                             row_image) \
                SELECT generation, writer, table_name, schema_version, 'c'::\"char\", \
                       jsonb_build_object('begin_seq', begin_seq) \
                  FROM done RETURNING seq) \
     SELECT seq FROM marker";

/// `$1 generation bigint, $2 writer text, $3 tables text[]`, or a null `$3`
/// for all of the writer's tables: delete those tables' log rows. One row per
/// table that had any, as `table_name, rows bigint`.
pub const DISCARD_SQL: &str = "WITH gone AS (DELETE FROM oxy_changes.log \
     WHERE generation = $1::bigint AND writer = $2::text \
       AND ($3::text[] IS NULL OR table_name = ANY($3::text[])) \
     RETURNING table_name) \
     SELECT table_name, count(*)::bigint AS rows FROM gone GROUP BY table_name \
     ORDER BY table_name";

/// `SET LOCAL lock_timeout`, in whole milliseconds and never zero: a zero
/// would wait forever.
pub fn lock_timeout_sql(timeout: Duration) -> String {
    format!(
        "SET LOCAL lock_timeout = '{}ms'",
        timeout.as_millis().max(1)
    )
}

/// The settings every transaction of a baseline's step starts with, on either
/// session, before any other statement: the search path pinned to
/// `pg_catalog, pg_temp`, then a lock timeout and a statement timeout, each in
/// whole milliseconds and never zero.
///
/// The writer's session would otherwise resolve unqualified functions and
/// operators (`to_jsonb`, `jsonb_each`, `=`, the key's `>`) through the app's
/// own schema, where an exact-type match the app defined, such as
/// `to_jsonb(<its table>)`, beats `pg_catalog`'s polymorphic one whatever the
/// path's order: an app could forge a platform-signed snapshot image, steer
/// the cursor, or hang the pass. Every relation and type the step's SQL names
/// is schema-qualified, so the pinned path reaches only `pg_catalog`. Each
/// setting is a `SET LOCAL`, a utility statement that takes no snapshot.
pub fn pin_session_sql(lock_timeout: Duration, statement_timeout: Duration) -> String {
    format!(
        "SET LOCAL search_path = pg_catalog, pg_temp; {}; \
         SET LOCAL statement_timeout = '{}ms'",
        lock_timeout_sql(lock_timeout),
        statement_timeout.as_millis().max(1)
    )
}

/// The isolation level a chunk's read runs at (see [`chunk_read_sql`]), for
/// the relay and the tests alike.
pub const CHUNK_READ_ISOLATION: IsolationLevel = IsolationLevel::RepeatableRead;

/// The begin's lock on `writer`'s `table`, taken by the writer's session: it
/// waits out the table's writers in flight and holds off new ones until the
/// `b` marker commits.
pub fn begin_lock_sql(writer: &str, table: &str) -> Result<String, SchemaError> {
    Ok(format!(
        "LOCK TABLE {} IN SHARE MODE",
        qualified(writer, table)?
    ))
}

/// `$1 oid` (a table's relid), `$2 text[]` (its key columns' names): each
/// named column's `attname` and `type`, the type qualified and quoted so it
/// names the same type whatever the search path, as
/// [`SnapshotShape::key_types`] takes it, then `lt`, `eq`, `ge` and `gt`: the
/// operators (btree strategies 1, 3, 4 and 5) of the type's default btree
/// operator class, each `<schema>.<op>`, as [`KeyOrder`] takes them.
///
/// The class is found as Postgres finds a default one: for the type's base
/// (a domain resolved through every domain it is over), its own class; else
/// the class of the pseudo-type its kind matches (`anyarray` for an array,
/// `anyrange`, `anymultirange`, `record` for a composite, `anyenum`), whose
/// operators are `pg_catalog`'s; else one it casts to without a conversion
/// (`varchar`'s is `text`'s). A type with none has null operators. The rows
/// come in `$2`'s order, the key's; a named column the table lacks has no
/// row, so the caller still checks that each key column got a type. Typmods
/// are dropped on purpose, since they change neither comparison nor
/// ordering. The writer's session can run it.
pub const KEY_TYPES_SQL: &str = "WITH RECURSIVE keyed AS ( \
       SELECT a.attname::text AS attname, a.atttypid, \
              pg_catalog.array_position($2::text[], a.attname::text) AS position \
         FROM pg_catalog.pg_attribute a \
        WHERE a.attrelid = $1::oid AND a.attname::text = ANY($2::text[]) \
          AND NOT a.attisdropped), \
     based (attname, oid, typtype, typbasetype) AS ( \
       SELECT k.attname, t.oid, t.typtype, t.typbasetype \
         FROM keyed k JOIN pg_catalog.pg_type t ON t.oid = k.atttypid \
       UNION ALL \
       SELECT b.attname, t.oid, t.typtype, t.typbasetype \
         FROM based b JOIN pg_catalog.pg_type t ON t.oid = b.typbasetype \
        WHERE b.typtype = 'd') \
     SELECT k.attname, \
       pg_catalog.quote_ident(n.nspname) || '.' || pg_catalog.quote_ident(t.typname) AS type, \
       o.lt, o.eq, o.ge, o.gt \
     FROM keyed k \
     JOIN pg_catalog.pg_type t ON t.oid = k.atttypid \
     JOIN pg_catalog.pg_namespace n ON n.oid = t.typnamespace \
     JOIN based b ON b.attname = k.attname AND b.typtype <> 'd' \
     JOIN pg_catalog.pg_type bt ON bt.oid = b.oid \
     LEFT JOIN LATERAL (SELECT oc.opcfamily, oc.opcintype FROM pg_catalog.pg_opclass oc \
       JOIN pg_catalog.pg_am am ON am.oid = oc.opcmethod \
      WHERE am.amname = 'btree' AND oc.opcdefault \
        AND (oc.opcintype = bt.oid \
             OR oc.opcintype = CASE \
                  WHEN bt.typcategory = 'A' THEN 'pg_catalog.anyarray'::pg_catalog.regtype \
                  WHEN bt.typtype = 'r' THEN 'pg_catalog.anyrange'::pg_catalog.regtype \
                  WHEN bt.typtype = 'm' THEN 'pg_catalog.anymultirange'::pg_catalog.regtype \
                  WHEN bt.typtype = 'c' THEN 'pg_catalog.record'::pg_catalog.regtype \
                  WHEN bt.typtype = 'e' THEN 'pg_catalog.anyenum'::pg_catalog.regtype END \
             OR oc.opcintype IN (SELECT ca.casttarget FROM pg_catalog.pg_cast ca \
                                  WHERE ca.castsource = bt.oid AND ca.castmethod = 'b')) \
      ORDER BY oc.opcintype = bt.oid DESC LIMIT 1) dc ON true \
     LEFT JOIN LATERAL (SELECT \
         pg_catalog.max(x.op) FILTER (WHERE x.strategy = 1) AS lt, \
         pg_catalog.max(x.op) FILTER (WHERE x.strategy = 3) AS eq, \
         pg_catalog.max(x.op) FILTER (WHERE x.strategy = 4) AS ge, \
         pg_catalog.max(x.op) FILTER (WHERE x.strategy = 5) AS gt \
       FROM (SELECT ao.amopstrategy AS strategy, \
                    pg_catalog.quote_ident(opn.nspname) || '.' || op.oprname AS op \
               FROM pg_catalog.pg_amop ao \
               JOIN pg_catalog.pg_operator op ON op.oid = ao.amopopr \
               JOIN pg_catalog.pg_namespace opn ON opn.oid = op.oprnamespace \
              WHERE ao.amopfamily = dc.opcfamily AND ao.amoplefttype = dc.opcintype \
                AND ao.amoprighttype = dc.opcintype) x) o ON true \
     ORDER BY k.position";

/// A key column's comparison operators, each `<schema>.<op>`, from its type's
/// default btree operator class, as [`KEY_TYPES_SQL`] reads them. A copy
/// orders and resumes by these, so its `ORDER BY` and its cursor compare in
/// one order: an operator resolved by name in the pinned session could fall
/// back to `pg_catalog`'s through an implicit cast for a type whose own
/// operators live elsewhere (`citext`'s), and the cursor would skip rows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyOrder {
    pub lt: String,
    pub eq: String,
    pub ge: String,
    pub gt: String,
}

impl KeyOrder {
    /// Every operator `<schema>.<op>`, the schema as `quote_ident` renders
    /// it and the operator only operator characters: nothing else reaches
    /// the SQL.
    fn is_valid(&self) -> bool {
        [&self.lt, &self.eq, &self.ge, &self.gt]
            .iter()
            .all(|op| qualified_operator(op))
    }
}

/// `<schema>.<op>`, as [`KEY_TYPES_SQL`] renders an operator.
fn qualified_operator(op: &str) -> bool {
    let Some((schema, name)) = op.rsplit_once('.') else {
        return false;
    };
    let operator = |c: char| "+-*/<>=~!@#%^&|`?".contains(c);
    !name.is_empty() && name.chars().all(operator) && rendered_identifier(schema)
}

/// An identifier as `quote_ident` renders one: plain lower case, or quoted
/// with every inner quote doubled.
fn rendered_identifier(s: &str) -> bool {
    if let Some(inner) = s.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        return !inner.is_empty() && !inner.replace("\"\"", "").contains('"');
    }
    let mut chars = s.chars();
    let first = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_');
    first && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '$')
}

/// What a chunk copies of a table, by name, as the baseline's shape version
/// records it.
#[derive(Debug, Clone, Copy)]
pub struct SnapshotShape<'a> {
    /// The captured columns: what each image keeps.
    pub captured: &'a [String],
    /// The primary key, in key order: what each key keeps, and what the copy
    /// orders and resumes by.
    pub primary_key: &'a [String],
    /// Each key column's type, parallel to `primary_key`, as
    /// [`KEY_TYPES_SQL`] reads it: what the cursor is read back as.
    pub key_types: &'a [String],
    /// Each key column's operators, parallel to `primary_key`, as
    /// [`KEY_TYPES_SQL`] reads them: what the copy orders and resumes by.
    pub key_order: &'a [KeyOrder],
}

/// What builds a snapshot row's image in the writer's session
/// ([`chunk_read_sql`]): `to_jsonb` itself, since the writer cannot execute
/// its definer image function, and a cast run there runs as the writer.
const WRITER_IMAGE: &str = "pg_catalog.to_jsonb";

/// One chunk of `writer`'s `table`, read `FOR SHARE` by the writer's session
/// in primary-key order: `key` and `image`, both as `jsonb` text. `$1 bigint`
/// is the limit, then, only when `after`, `$2` is the last key copied, as
/// `jsonb` text bound as text, and the chunk starts past it.
///
/// The cursor is read back from the key's columns alone, each as its own
/// type, so no other column's type sees it: a `NOT NULL` domain elsewhere in
/// the row would refuse the null a whole-row read gives it. The chunk orders
/// by each key column's own `<` and resumes past the cursor with its own `>`
/// ([`KeyOrder`]): as one row comparison, which the key's index seeks to,
/// when the key's operators are one schema's, else lexicographically under a
/// leading `>=` ([`past_cursor`]). A shape with no key, or without one type
/// and one set of operators per key column, is refused.
///
/// Run it at [`CHUNK_READ_ISOLATION`], `REPEATABLE READ`, in a transaction
/// pinned by [`pin_session_sql`]: at READ COMMITTED, a row whose key changes
/// while the read waits on it is re-fetched by EvalPlanQual, which re-checks
/// the `WHERE` but not the `ORDER BY`, so the new key past the cursor would be
/// returned and the keys between it and the cursor skipped. At `REPEATABLE
/// READ` the read fails with 40001 instead, and the chunk is read again.
pub fn chunk_read_sql(
    writer: &str,
    table: &str,
    shape: &SnapshotShape<'_>,
    after: bool,
) -> Result<String, SchemaError> {
    let target = qualified(writer, table)?;
    check_key(table, shape)?;
    let key = text_array(shape.primary_key.iter().map(String::as_str));
    let image = text_array(shape.captured.iter().map(String::as_str));
    let resume = if after {
        format!("\n WHERE {}", past_cursor(shape))
    } else {
        String::new()
    };
    // The writer's own session, which cannot execute its image function: a
    // cast `to_jsonb` reaches here runs as the writer, with its privileges.
    Ok(format!(
        "SELECT {}::text AS key,\n       {}::text AS image\n  FROM {target} r{resume}\n \
         ORDER BY {} LIMIT $1 FOR SHARE OF r",
        filtered(WRITER_IMAGE, "r", &key),
        filtered(WRITER_IMAGE, "r", &image),
        ordered(shape),
    ))
}

/// A copy orders and resumes by the key, so it needs one, and a type and
/// its operators for each of its columns.
fn check_key(table: &str, shape: &SnapshotShape<'_>) -> Result<(), SchemaError> {
    let columns = shape.primary_key.len();
    let reason = if columns == 0 {
        "it has no primary key"
    } else if shape.key_types.len() != columns {
        "each key column needs exactly one type"
    } else if shape.key_order.len() != columns || !shape.key_order.iter().all(KeyOrder::is_valid) {
        "each key column's type needs a default btree operator class"
    } else {
        return Ok(());
    };
    Err(SchemaError::InvalidKey {
        table: table.to_string(),
        reason,
    })
}

/// `r."k1" USING OPERATOR(<schema>.<), …`: the key's columns, each ordered
/// by its own type's `<`.
fn ordered(shape: &SnapshotShape<'_>) -> String {
    shape
        .primary_key
        .iter()
        .zip(shape.key_order)
        .map(|(k, o)| format!("r.{} USING OPERATOR({})", quote_ident(k), o.lt))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The rows past the cursor in key order, by each column's own operators:
/// `r.k1 > c.k1` for one column; for more, whose operators are all one
/// schema's (always so for built-in types), the row `(r.k1, r.k2, …) > (c.k1,
/// c.k2, …)` by that schema's `>` ([`row_past_cursor`]), which the key's index
/// seeks to; only for a key mixing schemas, `r.k1 >= c.k1 AND (r.k1 > c.k1 OR
/// (r.k1 = c.k1 AND r.k2 > c.k2) …)`, the leading `>=` a range of the index.
/// Each `c.k` is the cursor's column, read by a subquery of its own.
fn past_cursor(shape: &SnapshotShape<'_>) -> String {
    let order = shape.key_order;
    if order.len() > 1 && order.iter().all(|o| o.gt == order[0].gt) {
        return row_past_cursor(shape);
    }
    let compare = |i: usize, op: &str| {
        let k = quote_ident(&shape.primary_key[i]);
        format!("r.{k} OPERATOR({op}) {}", cursor_column(shape, &k))
    };
    let lexicographic = (0..order.len())
        .map(|i| {
            let mut terms: Vec<String> = (0..i).map(|j| compare(j, &order[j].eq)).collect();
            terms.push(compare(i, &order[i].gt));
            match terms.as_slice() {
                [one] => one.clone(),
                _ => format!("({})", terms.join(" AND ")),
            }
        })
        .collect::<Vec<_>>();
    match lexicographic.as_slice() {
        [one] => one.clone(),
        _ => format!(
            "{} AND ({})",
            compare(0, &order[0].ge),
            lexicographic.join(" OR ")
        ),
    }
}

/// `(r."k1", r."k2") OPERATOR(<schema>.>) (SELECT c."k1", c."k2" FROM
/// jsonb_to_record($2::text::jsonb) AS c(…))`: the rows past the cursor as one
/// row comparison by the key's one `>`, which the key's index seeks to.
fn row_past_cursor(shape: &SnapshotShape<'_>) -> String {
    let columns = |alias: &str| {
        shape
            .primary_key
            .iter()
            .map(|k| format!("{alias}.{}", quote_ident(k)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "({}) OPERATOR({}) (SELECT {} FROM jsonb_to_record($2::text::jsonb) AS c({}))",
        columns("r"),
        shape.key_order[0].gt,
        columns("c"),
        cursor_columns(shape)
    )
}

/// `(SELECT c."k" FROM jsonb_to_record($2::text::jsonb) AS c(…))`: the
/// cursor's column `k` (quoted), as its own type.
fn cursor_column(shape: &SnapshotShape<'_>, k: &str) -> String {
    format!(
        "(SELECT c.{k} FROM jsonb_to_record($2::text::jsonb) AS c({}))",
        cursor_columns(shape)
    )
}

/// `"k1" <type1>, "k2" <type2>`: the cursor record's column definitions.
fn cursor_columns(shape: &SnapshotShape<'_>) -> String {
    shape
        .primary_key
        .iter()
        .zip(shape.key_types)
        .map(|(k, t)| format!("{} {t}", quote_ident(k)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `"app_<writer>"."<table>"`, both names validated.
fn qualified(writer: &str, table: &str) -> Result<String, SchemaError> {
    let schema = WriterRef::app(writer)?.schema_name();
    validate_name(table)?;
    Ok(format!("{}.{}", quote_ident(&schema), quote_ident(table)))
}

#[cfg(test)]
mod tests;
