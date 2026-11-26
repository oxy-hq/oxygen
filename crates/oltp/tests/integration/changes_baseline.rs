//! A baseline's tenant side against a real Postgres (OLTP change stream
//! design §5.5): how requests coalesce, when a begin may run, and that the
//! locks order a baseline's marker and snapshot rows against the table's own
//! changes.
//!
//! Every begin and chunk here runs the relay's two sessions as the relay does,
//! each transaction pinned by [`pin_session_sql`]: a begin locks its requested
//! row as the owner, then takes the table's lock as the writer, then writes
//! its marker as the owner; a chunk reads its rows as the writer, at
//! [`CHUNK_READ_ISOLATION`], then writes them as the owner. Either way the
//! owner commits first, then the writer's commit releases the lock. The owner
//! cannot take the lock itself, and the writer cannot write the log.

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio_postgres::Transaction;

use oxy_oltp::changes::{
    BACKLOG_SQL, BACKOFF_SQL, BEGIN_SQL, BaselineReason, CHUNK_READ_ISOLATION, CHUNK_WRITE_SQL,
    COMPLETE_SQL, CaptureShape, DISCARD_SQL, KEY_TYPES_SQL, KeyOrder, LOCK_REQUEST_SQL,
    OPEN_BASELINES_SQL, REQUEST_MISSING_SQL, SnapshotShape, abandon_baselines_sql, begin_lock_sql,
    chunk_read_sql, owner_commit_sql, owner_uninstall_sql, pin_session_sql, request_baseline_sql,
};
use oxy_oltp::platform::{
    CHANGES_BASELINES_TABLE, CHANGES_LOG_TABLE, CHANGES_SHAPES_TABLE, CHANGES_STREAM_TABLE,
};

use crate::changes_capture::Store;
use crate::changes_step::admin_dsn_for;
use crate::{attempt, render_pg_error};

/// The table most baselines here are of.
const TABLE: &str = "leases";

/// Long enough that no wait a test means to make times out; the relay's is
/// one second.
const LOCK_TIMEOUT: Duration = Duration::from_secs(10);

/// Long enough that no statement a test runs times out; the relay's is ten
/// seconds.
const STATEMENT_TIMEOUT: Duration = Duration::from_secs(30);

/// What a test's step transaction starts with, as the relay's do.
fn pin() -> String {
    pin_session_sql(LOCK_TIMEOUT, STATEMENT_TIMEOUT)
}

/// How long [`blocked`] polls before it fails the test, so a lock that stops
/// blocking fails rather than hangs.
const BLOCKED_WITHIN: Duration = Duration::from_secs(10);

/// What the owner's session signs a chunk with; the MAC is the relay's
/// business, not this SQL's.
const PROVENANCE: [u8; 32] = [0xab; 32];

/// A writer's change to a row a chunk holds `FOR SHARE`.
const UPDATE_2: &str = "UPDATE leases SET name = 'x' WHERE id = 2";

/// A writer's change of a row's key, past the rows after it.
const REKEY_2: &str = "UPDATE leases SET id = 10 WHERE id = 2";

async fn connect(dsn: &str) -> Result<tokio_postgres::Client, String> {
    oxy_oltp::connect::connect(dsn, "changes baseline test")
        .await
        .map_err(render_pg_error)
}

fn text(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// What a begin needs of a [`Store`], owned, so one can run in its own task.
#[derive(Clone)]
struct Sessions {
    writer: String,
    writer_dsn: String,
    owner_dsn: String,
}

impl Sessions {
    fn of(store: &Store) -> Self {
        Self {
            writer: store.writer.clone(),
            writer_dsn: store.writer_dsn.clone(),
            owner_dsn: store.fx.owner_dsn.clone(),
        }
    }
}

/// `sql` in `t`.
async fn run(t: &Transaction<'_>, sql: &str) -> Result<(), String> {
    t.batch_execute(sql).await.map_err(render_pg_error)
}

/// The owner's first statements of a begin: pinned, then baseline `id`'s
/// row locked ([`LOCK_REQUEST_SQL`]). Whether it is still requested.
async fn lock_requested(marker: &Transaction<'_>, id: i64) -> Result<bool, String> {
    run(marker, &pin()).await?;
    let row = marker
        .query_opt(LOCK_REQUEST_SQL, &[&id])
        .await
        .map_err(render_pg_error)?;
    Ok(row.is_some())
}

/// The relay's begin of baseline `id` of `table`, in the relay's order: the
/// owner's session locks the requested row ([`LOCK_REQUEST_SQL`]); then the
/// writer's takes the table's `SHARE` lock; then the owner's runs
/// [`BEGIN_SQL`] as a statement of its own and commits, and the writer's
/// commits after it. Each transaction is pinned first. `Some((begin_seq,
/// schema_version))`, or `None` when the begin may not run.
async fn begin(sessions: &Sessions, table: &str, id: i64) -> Result<Option<(i64, i32)>, String> {
    let mut writer = connect(&sessions.writer_dsn).await?;
    let mut owner = connect(&sessions.owner_dsn).await?;
    let lock = begin_lock_sql(&sessions.writer, table).map_err(text)?;
    let marker = owner.transaction().await.map_err(render_pg_error)?;
    if !lock_requested(&marker, id).await? {
        marker.rollback().await.map_err(render_pg_error)?;
        return Ok(None);
    }
    let locked = writer.transaction().await.map_err(render_pg_error)?;
    run(&locked, &pin()).await?;
    // On its own, so `blocked` finds exactly it waiting.
    run(&locked, &lock).await?;
    let row = marker
        .query_opt(BEGIN_SQL, &[&id])
        .await
        .map_err(render_pg_error)?;
    marker.commit().await.map_err(render_pg_error)?;
    locked.commit().await.map_err(render_pg_error)?;
    Ok(row.map(|r| (r.get(0), r.get(1))))
}

/// [`begin`], which must run: its `begin_seq`.
async fn begun(store: &Store, table: &str, id: i64) -> Result<i64, String> {
    let (begin_seq, _) = begin(&Sessions::of(store), table, id)
        .await?
        .ok_or_else(|| "the begin did not run".to_string())?;
    Ok(begin_seq)
}

/// Request a baseline of `capture`'s table, the three statements in one
/// transaction, and return the id of its requested row.
async fn request(store: &Store, capture: &CaptureShape) -> Result<i64, String> {
    let relid = capture.shape.relid;
    store
        .as_owner(&request_baseline_sql(relid, BaselineReason::OptIn).join("; "))
        .await?;
    store
        .owner_count(&format!(
            "SELECT id FROM {CHANGES_BASELINES_TABLE} \
             WHERE relid = {relid}::oid AND state = 'requested'"
        ))
        .await
}

/// What the relay's writer session copies of a table, owned.
struct Copied {
    relid: u32,
    table: String,
    captured: Vec<String>,
    primary_key: Vec<String>,
    key_types: Vec<String>,
    key_order: Vec<KeyOrder>,
}

impl Copied {
    fn shape(&self) -> SnapshotShape<'_> {
        SnapshotShape {
            captured: &self.captured,
            primary_key: &self.primary_key,
            key_types: &self.key_types,
            key_order: &self.key_order,
        }
    }
}

/// A key column's operators in `schema`, as [`KEY_TYPES_SQL`] reads them.
fn operators(schema: &str) -> KeyOrder {
    KeyOrder {
        lt: format!("{schema}.<"),
        eq: format!("{schema}.="),
        ge: format!("{schema}.>="),
        gt: format!("{schema}.>"),
    }
}

/// `capture`'s table as a copy reads it, its key's types and operators read
/// by [`KEY_TYPES_SQL`] in the writer's session and put in the key's order,
/// as the relay does.
async fn copied(store: &Store, capture: &CaptureShape) -> Result<Copied, String> {
    let shape = &capture.shape;
    let client = connect(&store.writer_dsn).await?;
    let rows = client
        .query(KEY_TYPES_SQL, &[&shape.relid, &shape.primary_key])
        .await
        .map_err(render_pg_error)?;
    let typed = |k: &String| {
        let row = rows
            .iter()
            .find(|r| r.get::<_, String>("attname") == *k)
            .ok_or_else(|| format!("no type for key column {k}"))?;
        let op = |name: &str| row.get::<_, Option<String>>(name).unwrap_or_default();
        let order = KeyOrder {
            lt: op("lt"),
            eq: op("eq"),
            ge: op("ge"),
            gt: op("gt"),
        };
        Ok::<_, String>((row.get::<_, String>("type"), order))
    };
    let (key_types, key_order) = shape
        .primary_key
        .iter()
        .map(typed)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .unzip();
    Ok(Copied {
        relid: shape.relid,
        table: shape.table.clone(),
        captured: capture.captured.iter().map(|c| c.name.clone()).collect(),
        primary_key: shape.primary_key.clone(),
        key_types,
        key_order,
    })
}

/// A baseline requested and begun.
struct Begun {
    copied: Copied,
    id: i64,
    begin_seq: i64,
}

/// Request and begin a baseline of `capture`'s table.
async fn begun_copy(store: &Store, capture: &CaptureShape) -> Result<Begun, String> {
    let id = request(store, capture).await?;
    let begin_seq = begun(store, &capture.shape.table, id).await?;
    Ok(Begun {
        copied: copied(store, capture).await?,
        id,
        begin_seq,
    })
}

/// `leases` installed with `rows` rows (`id` 1.., `name` `Flat <id>`), and a
/// baseline of it requested and begun.
async fn begun_with_rows(store: &Store, rows: i64) -> Result<Begun, String> {
    let capture = store.leases(&[]).await?;
    store
        .as_writer(&format!(
            "INSERT INTO leases SELECT g, 'Flat ' || g, 'f' || g || '@x' \
             FROM generate_series(1, {rows}) g"
        ))
        .await?;
    begun_copy(store, &capture).await
}

/// Create `table` as the writer with `ddl`, and install its capture at
/// version 1.
async fn installed(store: &Store, table: &str, ddl: &str) -> Result<CaptureShape, String> {
    store.as_writer(ddl).await?;
    let capture = store.shape(table, &[]).await?;
    store.install(&capture, 1).await?;
    Ok(capture)
}

/// Wait until a session, running exactly `sql`, waits on a lock. `dsn` must
/// be that session's role: `pg_stat_activity` shows a role's wait events only
/// to that role (or a superuser).
pub(crate) async fn blocked(dsn: &str, sql: &str) -> Result<(), String> {
    let client = connect(dsn).await?;
    let deadline = Instant::now() + BLOCKED_WITHIN;
    loop {
        let row = client
            .query_one(
                "SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() \
                 AND wait_event_type = 'Lock' AND query = $1",
                &[&sql],
            )
            .await
            .map_err(render_pg_error)?;
        if row.get::<_, i64>(0) > 0 {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(format!("no session waited on a lock running {sql:?}"));
        }
        tokio::task::yield_now().await;
    }
}

/// One chunk, as the writer's session reads it: keys and images as `jsonb`
/// text.
#[derive(Debug)]
struct Chunk {
    keys: Vec<String>,
    images: Vec<String>,
}

/// Read up to `limit` rows of `copied`'s table `FOR SHARE` in `session`,
/// past the cursor `after` (`jsonb` text) when given, as the relay's writer
/// session does.
async fn read_chunk(
    session: &Transaction<'_>,
    store: &Store,
    copied: &Copied,
    after: Option<&str>,
    limit: i64,
) -> Result<Chunk, String> {
    let sql = chunk_read_sql(
        &store.writer,
        &copied.table,
        &copied.shape(),
        after.is_some(),
    )
    .map_err(text)?;
    session
        .batch_execute(&pin())
        .await
        .map_err(render_pg_error)?;
    let rows = match after {
        Some(key) => session.query(sql.as_str(), &[&limit, &key]).await,
        None => session.query(sql.as_str(), &[&limit]).await,
    }
    .map_err(render_pg_error)?;
    Ok(Chunk {
        keys: rows.iter().map(|r| r.get("key")).collect(),
        images: rows.iter().map(|r| r.get("image")).collect(),
    })
}

/// A chunk's transaction on the writer's `client`, at the isolation level the
/// relay reads a chunk at ([`CHUNK_READ_ISOLATION`]).
async fn chunk_session(client: &mut tokio_postgres::Client) -> Result<Transaction<'_>, String> {
    client
        .build_transaction()
        .isolation_level(CHUNK_READ_ISOLATION)
        .start()
        .await
        .map_err(render_pg_error)
}

/// [`read_chunk`] in a transaction of its own, committed at once.
async fn read_chunk_alone(
    store: &Store,
    copied: &Copied,
    after: Option<&str>,
    limit: i64,
) -> Result<Chunk, String> {
    let mut writer = connect(&store.writer_dsn).await?;
    let session = chunk_session(&mut writer).await?;
    let chunk = read_chunk(&session, store, copied, after, limit).await?;
    session.commit().await.map_err(render_pg_error)?;
    Ok(chunk)
}

/// [`CHUNK_WRITE_SQL`] for `chunk` in an owner transaction, committed or
/// rolled back, its last key bound as the text the read gave: `(live,
/// written)`.
async fn write_chunk(
    store: &Store,
    id: i64,
    chunk: &Chunk,
    commit: bool,
) -> Result<(i64, i64), String> {
    let last: &str = chunk.keys.last().ok_or("an empty chunk")?;
    let mac: &[u8] = &PROVENANCE;
    let mut owner = connect(&store.fx.owner_dsn).await?;
    let session = owner.transaction().await.map_err(render_pg_error)?;
    let row = session
        .query_one(
            CHUNK_WRITE_SQL,
            &[&id, &last, &chunk.keys, &chunk.images, &mac],
        )
        .await
        .map_err(render_pg_error)?;
    let ended = if commit {
        session.commit().await
    } else {
        session.rollback().await
    };
    ended.map_err(render_pg_error)?;
    Ok((row.get("live"), row.get("written")))
}

/// Baseline `id`'s cursor as text, read from [`OPEN_BASELINES_SQL`] as the
/// relay reads it.
async fn open_last_key(store: &Store, id: i64) -> Result<Option<String>, String> {
    let client = connect(&store.fx.owner_dsn).await?;
    let rows = client
        .query(OPEN_BASELINES_SQL, &[])
        .await
        .map_err(render_pg_error)?;
    let row = rows
        .iter()
        .find(|r| r.get::<_, i64>("id") == id)
        .ok_or_else(|| format!("baseline {id} is not open"))?;
    Ok(row.get("last_key"))
}

/// Copy `copied`'s table for baseline `id` in chunks of `limit`, each read
/// past the cursor the baseline recorded, until a chunk comes back empty:
/// how many chunks were copied. A copy that never ends fails.
async fn copy_all(store: &Store, copied: &Copied, id: i64, limit: i64) -> Result<usize, String> {
    for chunks in 0..10 {
        let after = open_last_key(store, id).await?;
        let chunk = read_chunk_alone(store, copied, after.as_deref(), limit).await?;
        if chunk.keys.is_empty() {
            return Ok(chunks);
        }
        write_chunk(store, id, &chunk, true).await?;
    }
    Err("the copy did not end".to_string())
}

/// One log row, with its `seq`.
#[derive(Debug)]
struct Entry {
    seq: i64,
    op: String,
    table_name: String,
    key: Option<Value>,
    image: Option<Value>,
    actor: Option<String>,
    provenance: Option<Vec<u8>>,
}

/// Every log row, in `seq` order.
async fn entries(store: &Store) -> Result<Vec<Entry>, String> {
    let client = connect(&store.fx.owner_dsn).await?;
    let sql = format!(
        "SELECT seq, op::text AS op, table_name, key, row_image, actor, provenance \
         FROM {CHANGES_LOG_TABLE} ORDER BY seq"
    );
    let rows = client
        .query(sql.as_str(), &[])
        .await
        .map_err(render_pg_error)?;
    Ok(rows
        .iter()
        .map(|r| Entry {
            seq: r.get("seq"),
            op: r.get("op"),
            table_name: r.get("table_name"),
            key: r.get("key"),
            image: r.get("row_image"),
            actor: r.get("actor"),
            provenance: r.get("provenance"),
        })
        .collect())
}

/// The `op` row keyed by `id`.
fn entry<'a>(log: &'a [Entry], op: &str, id: i64) -> &'a Entry {
    log.iter()
        .find(|e| e.op == op && e.key == Some(json!({"id": id})))
        .unwrap_or_else(|| panic!("no {op} row for id {id}: {log:#?}"))
}

fn seq_of(log: &[Entry], op: &str, id: i64) -> i64 {
    entry(log, op, id).seq
}

/// Every snapshot row's key, as `jsonb` text, in `seq` order.
async fn snapshot_keys(store: &Store) -> Result<Vec<String>, String> {
    let client = connect(&store.fx.owner_dsn).await?;
    let sql = format!("SELECT key::text FROM {CHANGES_LOG_TABLE} WHERE op = 's' ORDER BY seq");
    let rows = client
        .query(sql.as_str(), &[])
        .await
        .map_err(render_pg_error)?;
    Ok(rows.iter().map(|r| r.get(0)).collect())
}

/// One `baselines` row.
#[derive(Debug)]
struct Baseline {
    state: String,
    copied: i64,
    last_key: Option<Value>,
}

/// Every `baselines` row, in `id` order.
async fn baselines(store: &Store) -> Result<Vec<Baseline>, String> {
    let client = connect(&store.fx.owner_dsn).await?;
    let sql = format!("SELECT state, copied, last_key FROM {CHANGES_BASELINES_TABLE} ORDER BY id");
    let rows = client
        .query(sql.as_str(), &[])
        .await
        .map_err(render_pg_error)?;
    Ok(rows
        .iter()
        .map(|r| Baseline {
            state: r.get("state"),
            copied: r.get("copied"),
            last_key: r.get("last_key"),
        })
        .collect())
}

fn states(baselines: &[Baseline]) -> Vec<&str> {
    baselines.iter().map(|b| b.state.as_str()).collect()
}

async fn snapshot_rows(store: &Store) -> Result<i64, String> {
    store
        .owner_count(&format!(
            "SELECT count(*) FROM {CHANGES_LOG_TABLE} WHERE op = 's'"
        ))
        .await
}

async fn generation(store: &Store) -> Result<i64, String> {
    store
        .owner_count(&format!("SELECT generation FROM {CHANGES_STREAM_TABLE}"))
        .await
}

/// Create `tenants` as the writer and install its capture at version 1.
async fn tenants(store: &Store) -> Result<(), String> {
    installed(
        store,
        "tenants",
        "CREATE TABLE tenants (id bigint PRIMARY KEY, name text)",
    )
    .await
    .map(|_| ())
}

/// `leases` and `tenants` installed, with two changes to `leases` and one
/// to `tenants` in the log.
async fn two_tables_written(store: &Store) -> Result<(), String> {
    store.leases(&[]).await?;
    tenants(store).await?;
    store
        .as_writer("INSERT INTO leases VALUES (1, 'a', 'a@x'), (2, 'b', 'b@x')")
        .await?;
    store.as_writer("INSERT INTO tenants VALUES (1, 'a')").await
}

// ── Who may take the lock ────────────────────────────────────────────────────

async fn owner_lock(store: &Store) -> Result<Result<(), String>, String> {
    store.leases(&[]).await?;
    Ok(store
        .as_owner(&format!(
            "BEGIN; LOCK TABLE {}.leases IN SHARE MODE; COMMIT",
            store.app.schema_name()
        ))
        .await)
}

/// Ruling 1: the lock is the writer's session's to take. `LOCK TABLE` outside
/// a transaction block fails before its privilege check, hence the `BEGIN`.
#[tokio::test]
async fn the_store_owner_cannot_lock_a_writer_owned_table() {
    let Some(store) = Store::create("bl_owner_lock").await else {
        return;
    };
    let out = owner_lock(&store).await;
    store.cleanup().await;

    let e = out.expect("create leases").unwrap_err();
    assert!(e.contains("42501"), "{e}");
}

// ── Requests ─────────────────────────────────────────────────────────────────

async fn coalesce(store: &Store) -> Result<(Vec<Baseline>, Vec<Baseline>), String> {
    let capture = store.leases(&[]).await?;
    request(store, &capture).await?;
    let id = request(store, &capture).await?;
    let coalesced = baselines(store).await?;
    begun(store, TABLE, id).await?;
    request(store, &capture).await?;
    Ok((coalesced, baselines(store).await?))
}

#[tokio::test]
async fn a_request_coalesces_and_a_new_one_abandons_a_copying_baseline() {
    let Some(store) = Store::create("bl_coalesce").await else {
        return;
    };
    let out = coalesce(&store).await;
    store.cleanup().await;

    let (coalesced, after) = out.expect("request, begin, request");
    assert_eq!(states(&coalesced), ["requested"], "{coalesced:#?}");
    assert_eq!(states(&after), ["abandoned", "requested"], "{after:#?}");
}

async fn missing(store: &Store) -> Result<[Vec<(String, String)>; 2], String> {
    let capture = store.leases(&[]).await?;
    tenants(store).await?;
    request(store, &capture).await?;
    let client = connect(&store.fx.owner_dsn).await?;
    let mut runs = [vec![], vec![]];
    for run in &mut runs {
        let rows = client
            .query(REQUEST_MISSING_SQL, &[])
            .await
            .map_err(render_pg_error)?;
        *run = rows.iter().map(|r| (r.get(0), r.get(1))).collect();
    }
    Ok(runs)
}

#[tokio::test]
async fn missing_baselines_are_requested_once() {
    let Some(store) = Store::create("bl_missing").await else {
        return;
    };
    let out = missing(&store).await;
    store.cleanup().await;

    let [first, second] = out.expect("install two tables and request the missing");
    assert_eq!(
        first,
        [(store.writer.clone(), "tenants".to_string())],
        "only the table without a baseline"
    );
    assert_eq!(second, [], "nothing is missing any more");
}

// ── When a begin may run ─────────────────────────────────────────────────────

/// Whether baseline `id` begins after `set`, as the owner; `reset` undoes it.
async fn begins_while(store: &Store, id: i64, set: &str, reset: &str) -> Result<bool, String> {
    store.as_owner(set).await?;
    let began = begin(&Sessions::of(store), TABLE, id).await?.is_some();
    store.as_owner(reset).await?;
    Ok(began)
}

/// Whether the begin ran: uninstalled, at a stale shape, re-stamping, then
/// with none of those.
async fn gated(store: &Store) -> Result<([bool; 3], bool), String> {
    let capture = store.leases(&[]).await?;
    let relid = capture.shape.relid;
    let id = request(store, &capture).await?;
    let uninstalled = begins_while(
        store,
        id,
        &owner_uninstall_sql(relid),
        &owner_commit_sql(relid, 1),
    )
    .await?;
    let newer = format!(
        "INSERT INTO {CHANGES_SHAPES_TABLE} \
         (relid, schema_version, columns, primary_key, excluded) \
         SELECT relid, 2, columns, primary_key, excluded FROM {CHANGES_SHAPES_TABLE} \
         WHERE relid = {relid}::oid"
    );
    let older = format!(
        "DELETE FROM {CHANGES_SHAPES_TABLE} WHERE relid = {relid}::oid AND schema_version = 2"
    );
    let stale = begins_while(store, id, &newer, &older).await?;
    let restamping = begins_while(
        store,
        id,
        &format!("UPDATE {CHANGES_STREAM_TABLE} SET restamping = 0"),
        &format!("UPDATE {CHANGES_STREAM_TABLE} SET restamping = NULL"),
    )
    .await?;
    let ready = begin(&Sessions::of(store), TABLE, id).await?.is_some();
    Ok(([uninstalled, stale, restamping], ready))
}

#[tokio::test]
async fn a_baseline_begins_only_on_an_installed_table_at_its_current_shape() {
    let Some(store) = Store::create("bl_gated").await else {
        return;
    };
    let out = gated(&store).await;
    store.cleanup().await;

    let ([uninstalled, stale, restamping], ready) = out.expect("request and begin");
    assert!(!uninstalled, "not while installed is false");
    assert!(!stale, "not while a newer shape is recorded");
    assert!(!restamping, "not while the stream is re-stamping");
    assert!(ready, "and then it begins");
}

/// An install's first transaction requests a reshape baseline and sets
/// `installed = false`, and stays open while a begin of the already
/// requested baseline arrives: what the begin returned, and the baselines.
async fn racing(store: &Store) -> Result<(Option<(i64, i32)>, Vec<Baseline>), String> {
    let capture = store.leases(&[]).await?;
    let relid = capture.shape.relid;
    let id = request(store, &capture).await?;
    let install = connect(&store.fx.owner_dsn).await?;
    let reshape = request_baseline_sql(relid, BaselineReason::Reshape).join("; ");
    install
        .batch_execute(&format!("BEGIN; {reshape}; {}", owner_uninstall_sql(relid)))
        .await
        .map_err(render_pg_error)?;
    let sessions = Sessions::of(store);
    let begin_task = tokio::spawn(async move { begin(&sessions, TABLE, id).await });
    blocked(&store.fx.owner_dsn, LOCK_REQUEST_SQL).await?;
    install
        .batch_execute("COMMIT")
        .await
        .map_err(render_pg_error)?;
    let began = begin_task.await.map_err(text)??;
    Ok((began, baselines(store).await?))
}

#[tokio::test]
async fn a_begin_racing_a_reshape_request_waits_and_sees_it() {
    let Some(store) = Store::create("bl_racing").await else {
        return;
    };
    let out = racing(&store).await;
    store.cleanup().await;

    let (began, baselines) = out.expect("a begin waits on an install's request");
    assert_eq!(
        began, None,
        "the begin saw the install's installed = false, not the old shape"
    );
    assert_eq!(
        states(&baselines),
        ["requested"],
        "the reshape coalesced into the one requested row: {baselines:#?}"
    );
}

// ── The begin's lock ─────────────────────────────────────────────────────────

/// A writer's insert of id 9 in flight while a begin waits for its lock.
struct InFlightSeen {
    begin_seq: i64,
    /// `b` rows in the log while the begin waited.
    markers_while_blocked: i64,
    /// The log after the begin and one chunk.
    log: Vec<Entry>,
}

async fn in_flight(store: &Store) -> Result<InFlightSeen, String> {
    let capture = store.leases(&[]).await?;
    let id = request(store, &capture).await?;
    let open = connect(&store.writer_dsn).await?;
    open.batch_execute("BEGIN; INSERT INTO leases VALUES (9, 'Flat 9', 'f9@x')")
        .await
        .map_err(render_pg_error)?;
    let sessions = Sessions::of(store);
    let begin_task = tokio::spawn(async move { begin(&sessions, TABLE, id).await });
    blocked(
        &store.writer_dsn,
        &begin_lock_sql(&store.writer, TABLE).map_err(text)?,
    )
    .await?;
    let markers_while_blocked = store
        .owner_count(&format!(
            "SELECT count(*) FROM {CHANGES_LOG_TABLE} WHERE op = 'b'"
        ))
        .await?;
    open.batch_execute("COMMIT")
        .await
        .map_err(render_pg_error)?;
    let (begin_seq, _) = begin_task
        .await
        .map_err(text)??
        .ok_or_else(|| "the begin did not run".to_string())?;
    copy_all(store, &copied(store, &capture).await?, id, 10).await?;
    Ok(InFlightSeen {
        begin_seq,
        markers_while_blocked,
        log: entries(store).await?,
    })
}

#[tokio::test]
async fn a_begin_waits_out_a_writer_in_flight() {
    let Some(store) = Store::create("bl_in_flight").await else {
        return;
    };
    let out = in_flight(&store).await;
    store.cleanup().await;

    let seen = out.expect("the begin waits, then runs");
    assert_eq!(seen.markers_while_blocked, 0, "no marker while it waited");
    let insert_seq = seq_of(&seen.log, "i", 9);
    assert!(
        seen.begin_seq > insert_seq,
        "{} {insert_seq}",
        seen.begin_seq
    );
    let snapshot = entry(&seen.log, "s", 9);
    assert!(snapshot.seq > seen.begin_seq, "{:#?}", seen.log);
}

// ── A chunk's row locks ──────────────────────────────────────────────────────

/// A writer's update of id 2 waits on the chunk's row lock until the chunk's
/// snapshot rows have committed.
async fn outranked(store: &Store) -> Result<Vec<Entry>, String> {
    let begun = begun_with_rows(store, 3).await?;
    let mut writer = connect(&store.writer_dsn).await?;
    let held = chunk_session(&mut writer).await?;
    let chunk = read_chunk(&held, store, &begun.copied, None, 10).await?;
    let dsn = store.writer_dsn.clone();
    let update = tokio::spawn(async move { attempt(&dsn, UPDATE_2).await });
    blocked(&store.writer_dsn, UPDATE_2).await?;
    write_chunk(store, begun.id, &chunk, true).await?;
    held.commit().await.map_err(render_pg_error)?;
    update.await.map_err(text)??;
    entries(store).await
}

#[tokio::test]
async fn a_change_after_a_chunk_outranks_its_snapshot() {
    let Some(store) = Store::create("bl_outrank").await else {
        return;
    };
    let out = outranked(&store).await;
    store.cleanup().await;

    let log = out.expect("copy a chunk under a waiting update");
    let snapshot = entry(&log, "s", 2);
    let update = entry(&log, "u", 2);
    assert!(snapshot.seq < update.seq, "{log:#?}");
    assert_eq!(snapshot.image.as_ref().unwrap()["name"], json!("Flat 2"));
    assert_eq!(snapshot.actor.as_deref(), Some("platform"), "{snapshot:?}");
    assert_eq!(
        snapshot.provenance.as_deref(),
        Some(PROVENANCE.as_slice()),
        "{snapshot:?}"
    );
    assert_eq!(update.image.as_ref().unwrap()["name"], json!("x"));
}

/// What a chunk read at `REPEATABLE READ` made of a key changed under it.
struct RekeyedSeen {
    /// The read that waited on the changed row: its keys, or its error.
    waited: Result<Vec<String>, String>,
    /// The same read again, in a fresh transaction: its keys.
    retried: Vec<Value>,
}

/// Rows 1 to 4, begun; a writer changes key 2 to 10 and holds its
/// transaction open while a first chunk of two reads `FOR SHARE`. The read
/// waits on row 2; then the change commits.
async fn rekeyed(store: &Store) -> Result<RekeyedSeen, String> {
    let begun = begun_with_rows(store, 4).await?;
    let app = connect(&store.writer_dsn).await?;
    app.batch_execute(&format!("BEGIN; {REKEY_2}"))
        .await
        .map_err(render_pg_error)?;
    let read_sql =
        chunk_read_sql(&store.writer, TABLE, &begun.copied.shape(), false).map_err(text)?;
    let mut writer = connect(&store.writer_dsn).await?;
    let held = chunk_session(&mut writer).await?;
    let read = read_chunk(&held, store, &begun.copied, None, 2);
    let commit = async {
        blocked(&store.writer_dsn, &read_sql).await?;
        app.batch_execute("COMMIT").await.map_err(render_pg_error)
    };
    let (waited, committed) = tokio::join!(read, commit);
    committed?;
    drop(held);
    let again = chunk_session(&mut writer).await?;
    let chunk = read_chunk(&again, store, &begun.copied, None, 2).await?;
    again.commit().await.map_err(render_pg_error)?;
    let retried = chunk
        .keys
        .iter()
        .map(|k| serde_json::from_str::<Value>(k).map_err(text))
        .collect::<Result<_, _>>()?;
    Ok(RekeyedSeen {
        waited: waited.map(|chunk| chunk.keys),
        retried,
    })
}

/// At READ COMMITTED, EvalPlanQual would hand the waiting read the row's new
/// version, key 10, as the chunk's second row, and the cursor would skip keys
/// 3 and 4 for good. At `REPEATABLE READ`, as the relay reads, the read fails
/// with 40001 instead, and the read again returns the keys in order.
#[tokio::test]
async fn a_key_changed_under_a_chunk_fails_the_read_instead_of_skipping_rows() {
    let Some(store) = Store::create("bl_rekeyed").await else {
        return;
    };
    let out = rekeyed(&store).await;
    store.cleanup().await;

    let seen = out.expect("read a chunk under a key change");
    let failed = seen
        .waited
        .expect_err("the read fails rather than return key 10 past the cursor");
    assert!(failed.starts_with("40001"), "{failed}");
    assert_eq!(
        seen.retried,
        [json!({ "id": 1 }), json!({ "id": 3 })],
        "in key order: 10 never comes before 3"
    );
}

/// What a rolled-back chunk left, what the same chunk committed left, and
/// what a read from the committed cursor finds.
struct CursorSeen {
    rolled_back: (Vec<Baseline>, i64),
    committed: (Vec<Baseline>, i64),
    rest: Vec<Value>,
}

async fn cursor(store: &Store) -> Result<CursorSeen, String> {
    let begun = begun_with_rows(store, 3).await?;
    let chunk = read_chunk_alone(store, &begun.copied, None, 2).await?;
    write_chunk(store, begun.id, &chunk, false).await?;
    let rolled_back = (baselines(store).await?, snapshot_rows(store).await?);
    write_chunk(store, begun.id, &chunk, true).await?;
    let committed = (baselines(store).await?, snapshot_rows(store).await?);
    let last_key = open_last_key(store, begun.id).await?;
    let rest = read_chunk_alone(store, &begun.copied, last_key.as_deref(), 2).await?;
    let rest = rest
        .keys
        .iter()
        .map(|k| serde_json::from_str::<Value>(k).map_err(text))
        .collect::<Result<_, _>>()?;
    Ok(CursorSeen {
        rolled_back,
        committed,
        rest,
    })
}

#[tokio::test]
async fn a_chunk_and_its_cursor_commit_together() {
    let Some(store) = Store::create("bl_cursor").await else {
        return;
    };
    let out = cursor(&store).await;
    store.cleanup().await;

    let seen = out.expect("write a chunk, rolled back then committed");
    let (baselines, rows) = &seen.rolled_back;
    assert_eq!(
        (baselines[0].copied, &baselines[0].last_key, *rows),
        (0, &None, 0),
        "rolled back, neither the cursor nor the rows: {baselines:#?}"
    );
    let (baselines, rows) = &seen.committed;
    assert_eq!(
        (baselines[0].copied, &baselines[0].last_key, *rows),
        (2, &Some(json!({"id": 2})), 2),
        "committed, both: {baselines:#?}"
    );
    assert_eq!(
        seen.rest,
        [json!({"id": 3})],
        "the next chunk starts past it"
    );
}

/// A `numeric` key wider than an f64: the cursor must carry every digit, or
/// the resume would skip or repeat the second row.
async fn wide_numeric(store: &Store) -> Result<(usize, Vec<String>), String> {
    let capture = installed(
        store,
        "prices",
        "CREATE TABLE prices (id numeric PRIMARY KEY, label text)",
    )
    .await?;
    store
        .as_writer(
            "INSERT INTO prices VALUES (12345678901234567.89, 'a'), (12345678901234567.90, 'b')",
        )
        .await?;
    let begun = begun_copy(store, &capture).await?;
    let chunks = copy_all(store, &begun.copied, begun.id, 1).await?;
    Ok((chunks, snapshot_keys(store).await?))
}

#[tokio::test]
async fn a_wide_numeric_key_resumes_past_exactly_the_last_row() {
    let Some(store) = Store::create("bl_numeric").await else {
        return;
    };
    let out = wide_numeric(&store).await;
    store.cleanup().await;

    let (chunks, keys) = out.expect("copy a numeric-keyed table a row at a time");
    assert_eq!(chunks, 2, "{keys:?}");
    assert_eq!(
        keys,
        [
            r#"{"id": 12345678901234567.89}"#,
            r#"{"id": 12345678901234567.90}"#
        ],
        "each row copied once, every digit kept"
    );
}

/// A `NOT NULL` domain on a column outside the key: reading the cursor back
/// must not run that column's type.
async fn domain(store: &Store) -> Result<(usize, i64), String> {
    let capture = installed(
        store,
        "tags",
        "CREATE DOMAIN required_text AS text NOT NULL; \
         CREATE TABLE tags (id bigint PRIMARY KEY, title required_text)",
    )
    .await?;
    store
        .as_writer("INSERT INTO tags VALUES (1, 'a'), (2, 'b'), (3, 'c')")
        .await?;
    let begun = begun_copy(store, &capture).await?;
    let chunks = copy_all(store, &begun.copied, begun.id, 1).await?;
    Ok((chunks, snapshot_rows(store).await?))
}

#[tokio::test]
async fn a_not_null_domain_column_does_not_stop_the_copy() {
    let Some(store) = Store::create("bl_domain").await else {
        return;
    };
    let out = domain(&store).await;
    store.cleanup().await;

    let (chunks, rows) = out.expect("copy a table with a NOT NULL domain column");
    assert_eq!(
        (chunks, rows),
        (3, 3),
        "every chunk after the first resumes"
    );
}

async fn abandoned(store: &Store) -> Result<((i64, i64), i64, Vec<Baseline>), String> {
    let begun = begun_with_rows(store, 2).await?;
    let chunk = read_chunk_alone(store, &begun.copied, None, 10).await?;
    store
        .as_owner(&abandon_baselines_sql(begun.copied.relid))
        .await?;
    let wrote = write_chunk(store, begun.id, &chunk, true).await?;
    Ok((wrote, snapshot_rows(store).await?, baselines(store).await?))
}

#[tokio::test]
async fn a_chunk_of_an_abandoned_baseline_writes_nothing() {
    let Some(store) = Store::create("bl_abandoned").await else {
        return;
    };
    let out = abandoned(&store).await;
    store.cleanup().await;

    let (wrote, rows, baselines) = out.expect("abandon a copying baseline, then write a chunk");
    assert_eq!(wrote, (0, 0), "(live, written)");
    assert_eq!(rows, 0, "no snapshot row");
    assert_eq!(states(&baselines), ["abandoned"]);
}

// ── Complete ─────────────────────────────────────────────────────────────────

/// [`COMPLETE_SQL`] as the owner: the `c` marker's `seq`, or `None`.
async fn complete(store: &Store, id: i64) -> Result<Option<i64>, String> {
    let client = connect(&store.fx.owner_dsn).await?;
    let row = client
        .query_opt(COMPLETE_SQL, &[&id])
        .await
        .map_err(render_pg_error)?;
    Ok(row.map(|r| r.get(0)))
}

/// A baseline of an empty table, begun and completed twice.
struct CompleteSeen {
    begin_seq: i64,
    c_seq: Option<i64>,
    again: Option<i64>,
    log: Vec<Entry>,
    baselines: Vec<Baseline>,
}

async fn completed(store: &Store) -> Result<CompleteSeen, String> {
    let begun = begun_with_rows(store, 0).await?;
    let c_seq = complete(store, begun.id).await?;
    let again = complete(store, begun.id).await?;
    Ok(CompleteSeen {
        begin_seq: begun.begin_seq,
        c_seq,
        again,
        log: entries(store).await?,
        baselines: baselines(store).await?,
    })
}

#[tokio::test]
async fn a_complete_marker_names_its_begin() {
    let Some(store) = Store::create("bl_complete").await else {
        return;
    };
    let out = completed(&store).await;
    store.cleanup().await;

    let seen = out.expect("begin and complete");
    let log = &seen.log;
    let ops: Vec<&str> = log.iter().map(|e| e.op.as_str()).collect();
    assert_eq!(ops, ["b", "c"], "{log:#?}");
    assert_eq!(log[0].seq, seen.begin_seq, "{log:#?}");
    let c = &log[1];
    assert_eq!(Some(c.seq), seen.c_seq, "{log:#?}");
    assert_eq!(c.image, Some(json!({"begin_seq": seen.begin_seq})));
    assert_eq!(states(&seen.baselines), ["complete"]);
    assert_eq!(seen.again, None, "a complete baseline completes no more");
}

// ── Backlog and discard ──────────────────────────────────────────────────────

/// [`BACKLOG_SQL`] with `held` (null: none) and `cap`.
async fn backlog(store: &Store, held: Option<&[&str]>, cap: i64) -> Result<i64, String> {
    let generation = generation(store).await?;
    let held: Option<Vec<String>> = held.map(|t| t.iter().map(|s| s.to_string()).collect());
    let client = connect(&store.fx.owner_dsn).await?;
    let row = client
        .query_one(BACKLOG_SQL, &[&generation, &store.writer, &held, &cap])
        .await
        .map_err(render_pg_error)?;
    Ok(row.get(0))
}

async fn backlogs(store: &Store) -> Result<[i64; 3], String> {
    two_tables_written(store).await?;
    Ok([
        backlog(store, None, 10).await?,
        backlog(store, Some(&[TABLE]), 10).await?,
        backlog(store, None, 2).await?,
    ])
}

#[tokio::test]
async fn the_backlog_counts_up_to_its_cap() {
    let Some(store) = Store::create("bl_backlog").await else {
        return;
    };
    let out = backlogs(&store).await;
    store.cleanup().await;

    let [all, unheld, capped] = out.expect("write two tables and count");
    assert_eq!(all, 3, "a null held list holds nothing");
    assert_eq!(unheld, 1, "a held table's rows are left out");
    assert_eq!(capped, 2, "never counted past the cap");
}

/// Discard with `tables` (null: all of the writer's): one `(table, rows)`
/// per table that had rows.
async fn discard(store: &Store, tables: Option<&[&str]>) -> Result<Vec<(String, i64)>, String> {
    let generation = generation(store).await?;
    let tables: Option<Vec<String>> = tables.map(|t| t.iter().map(|s| s.to_string()).collect());
    let client = connect(&store.fx.owner_dsn).await?;
    let rows = client
        .query(DISCARD_SQL, &[&generation, &store.writer, &tables])
        .await
        .map_err(render_pg_error)?;
    Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
}

type DiscardSeen = (Vec<(String, i64)>, Vec<String>, Vec<(String, i64)>);

async fn discarded(store: &Store) -> Result<DiscardSeen, String> {
    two_tables_written(store).await?;
    let named = discard(store, Some(&[TABLE])).await?;
    let left = entries(store).await?;
    let all = discard(store, None).await?;
    Ok((named, left.into_iter().map(|e| e.table_name).collect(), all))
}

#[tokio::test]
async fn discard_deletes_only_the_named_tables_rows() {
    let Some(store) = Store::create("bl_discard").await else {
        return;
    };
    let out = discarded(&store).await;
    store.cleanup().await;

    let (named, left, all) = out.expect("write two tables and discard");
    assert_eq!(named, [("leases".to_string(), 2)]);
    assert_eq!(left, ["tenants"], "the other table's rows stay");
    assert_eq!(
        all,
        [("tenants".to_string(), 1)],
        "a null list is every table"
    );
}

// ── The pinned session ───────────────────────────────────────────────────────

/// An app's own `to_jsonb` for its table, created by the writer in its own
/// schema, as an app migration could: it forges every row's image.
const FORGED_TO_JSONB: &str = "CREATE FUNCTION to_jsonb(leases) RETURNS jsonb LANGUAGE sql \
     AS $$ SELECT '{\"id\": 1, \"name\": \"forged\"}'::jsonb $$";

/// One row, begun, then the forging `to_jsonb` in the app's schema: a chunk's
/// images as a pinned session reads them.
async fn forged(store: &Store) -> Result<Vec<String>, String> {
    let begun = begun_with_rows(store, 1).await?;
    store.as_writer(FORGED_TO_JSONB).await?;
    let pinned = read_chunk_alone(store, &begun.copied, None, 10).await?;
    Ok(pinned.images)
}

/// An app may define `to_jsonb(<its table>)` in its own schema; the pinned
/// path never reaches it, so the snapshot keeps the row's real image.
#[tokio::test]
async fn an_app_to_jsonb_does_not_reach_the_snapshot() {
    let Some(store) = Store::create("bl_forged").await else {
        return;
    };
    let out = forged(&store).await;
    store.cleanup().await;

    let pinned = out.expect("read a chunk past an app's to_jsonb");
    let image: Value = serde_json::from_str(&pinned[0]).expect("an image");
    assert_eq!(image["name"], json!("Flat 1"), "{pinned:?}");
}

/// The settings a pinned transaction shows, and a statement that outlasts
/// its statement timeout, as its error.
async fn pinned_settings(store: &Store) -> Result<(Vec<String>, String), String> {
    let mut writer = connect(&store.writer_dsn).await?;
    let session = writer.transaction().await.map_err(render_pg_error)?;
    session
        .batch_execute(&pin_session_sql(LOCK_TIMEOUT, Duration::from_millis(200)))
        .await
        .map_err(render_pg_error)?;
    let mut shown = Vec::new();
    for setting in ["search_path", "lock_timeout", "statement_timeout"] {
        let row = session
            .query_one(format!("SHOW {setting}").as_str(), &[])
            .await
            .map_err(render_pg_error)?;
        shown.push(row.get(0));
    }
    let slept = session
        .batch_execute("SELECT pg_catalog.pg_sleep(5)")
        .await
        .map_err(render_pg_error);
    Ok((shown, slept.err().unwrap_or_default()))
}

/// With the path pinned, no function or operator the app defines is reached
/// at all ([`an_app_to_jsonb_does_not_reach_the_snapshot`]), so there is no
/// slow app function left to call: this pins the timeout itself. A pinned
/// transaction shows the pinned path and both timeouts, and a statement that
/// outlasts its timeout is cut off with 57014.
#[tokio::test]
async fn a_slow_app_function_is_cut_off_by_the_statement_timeout() {
    let Some(store) = Store::create("bl_timeout").await else {
        return;
    };
    let out = pinned_settings(&store).await;
    store.cleanup().await;

    let (shown, slept) = out.expect("pin a session");
    assert_eq!(shown, ["pg_catalog, pg_temp", "10s", "200ms"]);
    assert!(slept.starts_with("57014"), "{slept:?}");
}

// ── Backoff ──────────────────────────────────────────────────────────────────

/// What [`BACKOFF_SQL`] and a begin made of a baseline's backoff.
struct BackoffSeen {
    /// `(attempts, delay)` after the first backoff and the second.
    first: (i32, f64),
    second: (i32, f64),
    /// Whether [`OPEN_BASELINES_SQL`] read it as backing off.
    backing_off: bool,
    /// `(attempts, delay)` of a backoff from 30 attempts.
    capped: (i32, f64),
    /// `(attempts, retry_at IS NULL)` after a begin.
    cleared: (i32, bool),
}

async fn back_off(owner: &tokio_postgres::Client, id: i64) -> Result<(i32, f64), String> {
    let row = owner
        .query_one(BACKOFF_SQL, &[&id])
        .await
        .map_err(render_pg_error)?;
    Ok((row.get("attempts"), row.get("delay")))
}

async fn backed_off(store: &Store) -> Result<BackoffSeen, String> {
    let capture = store.leases(&[]).await?;
    let id = request(store, &capture).await?;
    let owner = connect(&store.fx.owner_dsn).await?;
    let first = back_off(&owner, id).await?;
    let second = back_off(&owner, id).await?;
    let open = owner
        .query(OPEN_BASELINES_SQL, &[])
        .await
        .map_err(render_pg_error)?;
    let backing_off = open
        .iter()
        .any(|r| r.get::<_, i64>("id") == id && r.get::<_, bool>("backing_off"));
    owner
        .execute(
            format!("UPDATE {CHANGES_BASELINES_TABLE} SET attempts = 30 WHERE id = $1").as_str(),
            &[&id],
        )
        .await
        .map_err(render_pg_error)?;
    let capped = back_off(&owner, id).await?;
    begun(store, TABLE, id).await?;
    let row = owner
        .query_one(
            format!(
                "SELECT attempts, retry_at IS NULL FROM {CHANGES_BASELINES_TABLE} WHERE id = $1"
            )
            .as_str(),
            &[&id],
        )
        .await
        .map_err(render_pg_error)?;
    Ok(BackoffSeen {
        first,
        second,
        backing_off,
        capped,
        cleared: (row.get(0), row.get(1)),
    })
}

/// A backoff waits `5 s · 2^attempts` from now, counting each one, at most 30
/// minutes; the open read sees it backing off; a begin clears it.
#[tokio::test]
async fn a_baseline_backs_off_and_a_begin_clears_it() {
    let Some(store) = Store::create("bl_backoff").await else {
        return;
    };
    let out = backed_off(&store).await;
    store.cleanup().await;

    let seen = out.expect("back a baseline off");
    assert_eq!(seen.first, (1, 5.0));
    assert_eq!(seen.second, (2, 10.0));
    assert!(
        seen.backing_off,
        "read as backing off while retry_at is ahead"
    );
    assert_eq!(seen.capped, (31, 1800.0), "at most 30 minutes");
    assert_eq!(seen.cleared, (0, true), "a begin clears the backoff");
}

// ── A key's own order ────────────────────────────────────────────────────────

/// The operators [`KEY_TYPES_SQL`] reads for a `bigint` key and for a
/// `varchar` one, whose default btree class is `text`'s, which it casts to
/// without a conversion.
async fn builtin_orders(store: &Store) -> Result<[Vec<KeyOrder>; 2], String> {
    let leases = store.leases(&[]).await?;
    let codes = installed(
        store,
        "codes",
        "CREATE TABLE codes (code varchar(8) PRIMARY KEY, label text)",
    )
    .await?;
    Ok([
        copied(store, &leases).await?.key_order,
        copied(store, &codes).await?.key_order,
    ])
}

#[tokio::test]
async fn a_key_orders_by_its_types_default_btree_operators() {
    let Some(store) = Store::create("bl_key_order").await else {
        return;
    };
    let out = builtin_orders(&store).await;
    store.cleanup().await;

    let [bigint, varchar] = out.expect("read the keys' operators");
    assert_eq!(bigint, [operators("pg_catalog")]);
    assert_eq!(
        varchar,
        [operators("pg_catalog")],
        "text's, through the cast"
    );
}

/// `citext` installed in a schema of its own that every role may use, as the
/// cluster superuser. `false` when the cluster has no `citext` to install.
async fn citext(store: &Store) -> Result<bool, String> {
    let admin = connect(&admin_dsn_for(&store.fx.database)).await?;
    let installed = admin
        .batch_execute(
            "CREATE SCHEMA IF NOT EXISTS ext; GRANT USAGE ON SCHEMA ext TO PUBLIC; \
             CREATE EXTENSION IF NOT EXISTS citext SCHEMA ext",
        )
        .await
        .map_err(render_pg_error);
    match installed {
        Ok(()) => Ok(true),
        Err(e) if e.starts_with("58P01") || e.starts_with("0A000") => Ok(false),
        Err(e) => Err(e),
    }
}

/// Four rows whose keys `citext` orders without case (`a`, `B`, `c`, `D`),
/// as text would not, copied one row a chunk: how many chunks, the snapshot
/// keys in `seq` order, and the key's operators. `None` without `citext`.
async fn citext_copied(
    store: &Store,
) -> Result<Option<(usize, Vec<String>, Vec<KeyOrder>)>, String> {
    if !citext(store).await? {
        return Ok(None);
    }
    let capture = installed(
        store,
        "tags",
        "CREATE TABLE tags (name ext.citext PRIMARY KEY, note text)",
    )
    .await?;
    store
        .as_writer("INSERT INTO tags VALUES ('a', '1'), ('B', '2'), ('c', '3'), ('D', '4')")
        .await?;
    let begun = begun_copy(store, &capture).await?;
    let chunks = copy_all(store, &begun.copied, begun.id, 1).await?;
    let order = begun.copied.key_order.clone();
    Ok(Some((chunks, snapshot_keys(store).await?, order)))
}

/// Ordered by `citext`'s own `<` but compared past the cursor by text's `>`,
/// the copy would read `a`, then skip `B` (text sorts it before `a`); by the
/// key type's own operators it copies each row once, in `citext`'s order.
#[tokio::test]
async fn a_citext_key_is_copied_once_in_its_own_order() {
    let Some(store) = Store::create("bl_citext").await else {
        return;
    };
    let out = citext_copied(&store).await;
    store.cleanup().await;

    let Some((chunks, keys, order)) = out.expect("copy a citext-keyed table") else {
        eprintln!("skipped: the cluster has no citext to install");
        return;
    };
    assert_eq!(order, [operators("ext")], "citext's own operators");
    assert_eq!(chunks, 4, "one row a chunk");
    assert_eq!(
        keys,
        [
            r#"{"name": "a"}"#,
            r#"{"name": "B"}"#,
            r#"{"name": "c"}"#,
            r#"{"name": "D"}"#
        ],
        "each row once, in citext's order"
    );
}

/// An array key and a key behind two domains, each with three rows, copied
/// one row a chunk: each one's operators and how many chunks it took, then
/// every snapshot key.
async fn unusual_keys(store: &Store) -> Result<(Vec<(Vec<KeyOrder>, usize)>, usize), String> {
    let grids = installed(
        store,
        "grids",
        "CREATE TABLE grids (cell bigint[] PRIMARY KEY, label text)",
    )
    .await?;
    store
        .as_writer("INSERT INTO grids VALUES ('{1,2}', 'a'), ('{1,3}', 'b'), ('{2}', 'c')")
        .await?;
    let coded = installed(
        store,
        "coded",
        "CREATE DOMAIN code AS bigint; CREATE DOMAIN short_code AS code; \
         CREATE TABLE coded (id short_code PRIMARY KEY, label text)",
    )
    .await?;
    store
        .as_writer("INSERT INTO coded VALUES (1, 'a'), (2, 'b'), (3, 'c')")
        .await?;
    let mut copied = Vec::new();
    for capture in [grids, coded] {
        let begun = begun_copy(store, &capture).await?;
        let chunks = copy_all(store, &begun.copied, begun.id, 1).await?;
        copied.push((begun.copied.key_order.clone(), chunks));
    }
    Ok((copied, snapshot_keys(store).await?.len()))
}

/// An array's default btree class is declared on `anyarray`, and a domain's
/// on its base's, however many domains deep: both order and resume by
/// `pg_catalog`'s operators, and copy each row once.
#[tokio::test]
async fn array_and_nested_domain_keys_copy_once() {
    let Some(store) = Store::create("bl_unusual_keys").await else {
        return;
    };
    let out = unusual_keys(&store).await;
    store.cleanup().await;

    let (copied, snapshots) = out.expect("copy both tables");
    for (order, chunks) in copied {
        assert_eq!(order, [operators("pg_catalog")]);
        assert_eq!(chunks, 3, "one row a chunk");
    }
    assert_eq!(snapshots, 6, "each row once");
}
