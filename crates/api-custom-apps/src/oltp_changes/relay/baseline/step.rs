//! A baseline's steps on the store's two sessions: its begin, and each chunk
//! of its copy (Ruling 1). **The order is the point.**
//!
//! - **A begin.** The owner's transaction locks the requested row
//!   ([`LOCK_REQUEST_SQL`]), waiting out an install's request in flight. Only
//!   then does the writer's queue the table's `SHARE` lock, so app writes
//!   never wait behind a begin that is itself waiting. Then [`BEGIN_SQL`] runs
//!   as the owner's next statement, on a snapshot taken after both locks.
//! - **A chunk.** The writer's session reads its rows `FOR SHARE`, at
//!   [`CHUNK_READ_ISOLATION`] ([`chunk_reader`]), then the owner's writes
//!   them. A read that meets a row changed since its snapshot fails (40001)
//!   and is tried again, at most [`CHUNK_ATTEMPTS`] times in all ([`retry`]).
//!
//! Every transaction of a step, on either session, starts pinned
//! ([`pinned`]): the search path to `pg_catalog`, so nothing the app's schema
//! defines is reached, and a lock and a statement timeout. Either way the
//! owner's transaction commits before the writer's ([`end`]).
//!
//! A lock that waits past [`BEGIN_LOCK_TIMEOUT`] on either session is
//! [`Halt::LockTimeout`]; a chunk whose reads all conflicted is
//! [`Halt::Conflict`]. Both back the baseline off rather than fail the
//! writer. Any other failure, a statement timeout (57014) included, is
//! [`Halt::Error`], which fails the writer for the pass naming the table.

use oxy_oltp::changes::{
    BEGIN_SQL, CHUNK_READ_ISOLATION, CHUNK_WRITE_SQL, COMPLETE_SQL, LOCK_REQUEST_SQL,
    PLATFORM_ACTOR, begin_lock_sql, chunk_read_sql, pin_session_sql,
};
use oxy_oltp::connect::pg_detail;
use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, Transaction};
use uuid::Uuid;

use super::{Open, Sessions, Shape};
use crate::oltp_changes::provenance::{self, ChangeSigner, TXID_SQL, parse_txid};
use crate::oltp_changes::relay::pass::{Pass, Stop};
use crate::oltp_changes::relay::{BASELINE_STATEMENT_TIMEOUT, BEGIN_LOCK_TIMEOUT, PRODUCTION};

const BEGIN: &str = "begin the baseline";
const CHUNK: &str = "copy a chunk";

/// How many times a pass tries a chunk whose read conflicts with a change to
/// its rows (40001) before it backs the baseline off.
const CHUNK_ATTEMPTS: u32 = 3;

/// How one step of a baseline ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Step {
    /// Its `b` marker written, under this shape version.
    Began(i32),
    /// It may not begin now ([`BEGIN_SQL`]).
    NotYet,
    /// A chunk of this many rows copied.
    Copied(u64),
    /// Its `c` marker written.
    Completed,
    /// A lock waited past [`BEGIN_LOCK_TIMEOUT`].
    LockTimeout,
    /// The chunk's rows kept changing under its reads, through every attempt.
    Conflicted,
    /// No longer copying: a request or a removal abandoned it.
    Abandoned,
    /// Its key's type has no default btree operator class to copy it by.
    Unordered,
}

/// Why a step's statement failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Halt {
    /// A lock waited past [`BEGIN_LOCK_TIMEOUT`] (55P03).
    LockTimeout,
    /// The chunk's read met a row changed since its snapshot (40001): the
    /// chunk may be tried again ([`retry`]).
    Conflict,
    Error(String),
}

/// A failure of `table`'s baseline, which fails its writer for the pass.
pub(super) fn failed_on(table: &str, error: String) -> Stop {
    Stop::Failed(format!("baseline of {table}: {error}"))
}

/// A step's outcome for its writer: a lock timeout and a chunk that kept
/// conflicting are steps of their own, which back the baseline off; any
/// other failure fails the writer, naming the table.
pub(super) fn settled(table: &str, outcome: Result<Step, Halt>) -> Result<Step, Stop> {
    match outcome {
        Ok(step) => Ok(step),
        Err(Halt::LockTimeout) => Ok(Step::LockTimeout),
        Err(Halt::Conflict) => Ok(Step::Conflicted),
        Err(Halt::Error(error)) => Err(failed_on(table, error)),
    }
}

/// A failed statement of a step: its lock timed out, or `doing` failed with
/// the server's message.
fn halt(doing: &'static str) -> impl Fn(tokio_postgres::Error) -> Halt {
    move |e| {
        halt_for(e.code()).unwrap_or_else(|| Halt::Error(format!("{doing}: {}", pg_detail(&e))))
    }
}

/// [`halt`] for a chunk's read, the one statement that can conflict.
fn read_halt(doing: &'static str) -> impl Fn(tokio_postgres::Error) -> Halt {
    move |e| {
        read_halt_for(e.code())
            .unwrap_or_else(|| Halt::Error(format!("{doing}: {}", pg_detail(&e))))
    }
}

/// The SQLSTATE every step handles by name: a lock timeout (55P03).
fn halt_for(code: Option<&SqlState>) -> Option<Halt> {
    (code == Some(&SqlState::LOCK_NOT_AVAILABLE)).then_some(Halt::LockTimeout)
}

/// The SQLSTATEs a chunk's read handles by name: a serialization failure
/// (40001), which only its [`CHUNK_READ_ISOLATION`] read meets; a lock
/// timeout; and a deadlock (40P01), which its row locks can meet with an app's
/// transaction, and which backs the baseline off as a lock timeout does.
fn read_halt_for(code: Option<&SqlState>) -> Option<Halt> {
    if code == Some(&SqlState::T_R_SERIALIZATION_FAILURE) {
        return Some(Halt::Conflict);
    }
    if code == Some(&SqlState::T_R_DEADLOCK_DETECTED) {
        return Some(Halt::LockTimeout);
    }
    halt_for(code)
}

/// Whether a chunk whose `attempt`th try halted is tried again: only after a
/// conflict, and only while attempts remain. A conflict on the last attempt
/// stays one, and backs the baseline off.
fn retry(halted: Halt, attempt: u32) -> Result<(), Halt> {
    match halted {
        Halt::Conflict if attempt < CHUNK_ATTEMPTS => Ok(()),
        other => Err(other),
    }
}

/// The step's settings, first in each of its transactions:
/// [`pin_session_sql`] with [`BEGIN_LOCK_TIMEOUT`] and
/// [`BASELINE_STATEMENT_TIMEOUT`].
async fn pinned(t: &Transaction<'_>) -> Result<(), Halt> {
    t.batch_execute(&pin_session_sql(
        BEGIN_LOCK_TIMEOUT,
        BASELINE_STATEMENT_TIMEOUT,
    ))
    .await
    .map_err(halt("pin the step's session"))
}

/// Begin `b`. The owner's transaction locks its requested row first, so the
/// writer's `SHARE` lock is never queued behind an install's request; then
/// the writer's takes the lock, and [`BEGIN_SQL`] writes the `b` marker on a
/// snapshot taken after both. [`end`] commits the owner's first.
pub(super) async fn begin(s: &mut Sessions<'_>, b: &Open) -> Result<Step, Halt> {
    let lock = begin_lock_sql(&s.name, &b.table)
        .map_err(|e| Halt::Error(format!("the begin's lock: {e}")))?;
    let owner = s.owner.transaction().await.map_err(halt(BEGIN))?;
    if !requested(&owner, b.id).await? {
        owner.rollback().await.map_err(halt(BEGIN))?;
        return Ok(Step::NotYet);
    }
    let writer = s.writer.transaction().await.map_err(halt(BEGIN))?;
    locked(&writer, &lock).await?;
    let begun = owner
        .query_opt(BEGIN_SQL, &[&b.id])
        .await
        .map_err(halt("write the begin marker"))?;
    let Some(row) = begun else {
        end(owner, writer, false).await?;
        return Ok(Step::NotYet);
    };
    let schema_version = row.try_get("schema_version").map_err(halt(BEGIN))?;
    end(owner, writer, true).await?;
    Ok(Step::Began(schema_version))
}

/// The owner's first statements of a begin: its settings, then baseline
/// `id`'s row locked ([`LOCK_REQUEST_SQL`]). Whether it is still requested.
async fn requested(owner: &Transaction<'_>, id: i64) -> Result<bool, Halt> {
    pinned(owner).await?;
    let row = owner
        .query_opt(LOCK_REQUEST_SQL, &[&id])
        .await
        .map_err(halt("lock the requested baseline"))?;
    Ok(row.is_some())
}

/// The writer's statements of a begin: its settings, then the table's
/// `SHARE` lock (`lock`).
async fn locked(writer: &Transaction<'_>, lock: &str) -> Result<(), Halt> {
    pinned(writer).await?;
    writer
        .batch_execute(lock)
        .await
        .map_err(halt("lock the table"))
}

/// One chunk of `b`, at most `limit` rows past its cursor ([`chunk_once`]),
/// tried again in the same pass while its read conflicts ([`retry`]).
pub(super) async fn chunk(
    pass: &Pass<'_>,
    s: &mut Sessions<'_>,
    b: &mut Open,
    shape: &Shape,
    limit: i64,
) -> Result<Step, Halt> {
    let mut attempt = 1;
    loop {
        match chunk_once(pass, s, b, shape, limit).await {
            Err(halted) => retry(halted, attempt)?,
            done => return done,
        }
        attempt += 1;
    }
}

/// One attempt at a chunk: the writer's session reads its rows `FOR SHARE`,
/// the owner's writes them as `s` rows, signed as the platform over its own
/// txid, and moves the cursor; [`end`] commits the owner's first. A read that
/// fails rolls back the writer's transaction before the owner's has begun. A
/// chunk that finds no rows completes `b`.
async fn chunk_once(
    pass: &Pass<'_>,
    s: &mut Sessions<'_>,
    b: &mut Open,
    shape: &Shape,
    limit: i64,
) -> Result<Step, Halt> {
    let writer = chunk_reader(&mut s.writer).await?;
    let (keys, images) = match read_chunk(&writer, &s.name, b, shape, limit).await {
        Ok(rows) => rows,
        Err(halted) => {
            writer.rollback().await.map_err(halt(CHUNK))?;
            return Err(halted);
        }
    };
    let Some(last) = keys.last().cloned() else {
        writer.rollback().await.map_err(halt(CHUNK))?;
        return complete(s.owner, b.id).await;
    };
    let owner = s.owner.transaction().await.map_err(halt(CHUNK))?;
    let signer = Signer {
        org_id: pass.org_id,
        schema: &s.schema,
    };
    let live = written(&owner, signer, b.id, &keys, &images).await?;
    end(owner, writer, live).await?;
    if !live {
        return Ok(Step::Abandoned);
    }
    b.last_key = Some(last);
    Ok(Step::Copied(keys.len() as u64))
}

/// Whom a chunk's snapshot rows are signed for: the org's key, and the
/// writer's schema.
#[derive(Clone, Copy)]
struct Signer<'a> {
    org_id: Uuid,
    schema: &'a str,
}

/// The owner's statements of a chunk, in its transaction: its settings, then
/// the `keys` and `images` written as `s` rows of baseline `id`, signed as
/// the platform over the transaction's own txid, and the cursor moved to the
/// last key. Whether the baseline was still copying; nothing was written
/// when it was not.
async fn written(
    owner: &Transaction<'_>,
    signer: Signer<'_>,
    id: i64,
    keys: &[String],
    images: &[String],
) -> Result<bool, Halt> {
    pinned(owner).await?;
    let mac = sign(signer.org_id, owner, signer.schema).await?;
    let last = keys.last().map(String::as_str).unwrap_or_default();
    let row = owner
        .query_one(
            CHUNK_WRITE_SQL,
            &[&id, &last, &keys, &images, &mac.as_slice()],
        )
        .await
        .map_err(halt("write the chunk"))?;
    let live: i64 = row.try_get("live").map_err(halt(CHUNK))?;
    Ok(live > 0)
}

/// A chunk's writer transaction, at [`CHUNK_READ_ISOLATION`]
/// (`REPEATABLE READ`): at READ COMMITTED a row whose key changed while the
/// read waited on it would be re-fetched by EvalPlanQual past the cursor,
/// skipping the keys between; here the read fails with 40001 instead. Its
/// settings are utility statements, which take no snapshot, so the snapshot
/// is the read's own.
async fn chunk_reader(writer: &mut Client) -> Result<Transaction<'_>, Halt> {
    let t = writer
        .build_transaction()
        .isolation_level(CHUNK_READ_ISOLATION)
        .start()
        .await
        .map_err(halt(CHUNK))?;
    pinned(&t).await?;
    Ok(t)
}

/// The chunk's rows, read `FOR SHARE` in `t`: each one's key and image, as
/// `jsonb` text.
async fn read_chunk(
    t: &Transaction<'_>,
    writer: &str,
    b: &Open,
    shape: &Shape,
    limit: i64,
) -> Result<(Vec<String>, Vec<String>), Halt> {
    let sql = chunk_read_sql(writer, &b.table, &shape.snapshot(), b.last_key.is_some())
        .map_err(|e| Halt::Error(format!("the chunk's read: {e}")))?;
    let rows = match &b.last_key {
        Some(after) => t.query(sql.as_str(), &[&limit, after]).await,
        None => t.query(sql.as_str(), &[&limit]).await,
    }
    .map_err(read_halt("read a chunk"))?;
    let column = |name: &str| {
        rows.iter()
            .map(|row| row.try_get::<_, String>(name))
            .collect::<Result<Vec<_>, _>>()
    };
    let keys = column("key").map_err(halt(CHUNK))?;
    let images = column("image").map_err(halt(CHUNK))?;
    Ok((keys, images))
}

/// The MAC of the owner's transaction's snapshot rows: over its own txid,
/// for the writer's `schema`, as [`PLATFORM_ACTOR`], with no invocation.
async fn sign(org_id: Uuid, t: &Transaction<'_>, schema: &str) -> Result<[u8; 32], Halt> {
    let doing = "read the copy's transaction id";
    let row = t.query_one(TXID_SQL, &[]).await.map_err(halt(doing))?;
    let text: String = row.try_get(0).map_err(halt(doing))?;
    let txid =
        parse_txid(&text).ok_or_else(|| Halt::Error(format!("{doing}: {text:?} is not one")))?;
    let signer = ChangeSigner {
        txid,
        environment: PRODUCTION,
        writer: schema,
        actor: PLATFORM_ACTOR,
        invocation: None,
    };
    Ok(provenance::sign(org_id, &signer))
}

/// Complete `b` (`id`), writing its `c` marker in a pinned owner transaction
/// of its own: [`Step::Abandoned`] when it is no longer copying.
async fn complete(owner: &mut Client, id: i64) -> Result<Step, Halt> {
    let doing = "complete the baseline";
    let t = owner.transaction().await.map_err(halt(doing))?;
    pinned(&t).await?;
    let row = t
        .query_opt(COMPLETE_SQL, &[&id])
        .await
        .map_err(halt(doing))?;
    t.commit().await.map_err(halt(doing))?;
    Ok(if row.is_some() {
        Step::Completed
    } else {
        Step::Abandoned
    })
}

/// End a step's two transactions, both committed or both rolled back, and
/// the owner's always first: its marker or rows commit, and draw their `seq`,
/// while the writer's locks still hold the table's writers off; only then
/// does the writer's commit release them.
async fn end(owner: Transaction<'_>, writer: Transaction<'_>, commit: bool) -> Result<(), Halt> {
    if !commit {
        owner
            .rollback()
            .await
            .map_err(halt("roll back the owner's step"))?;
        return writer
            .rollback()
            .await
            .map_err(halt("roll back the writer's step"));
    }
    owner
        .commit()
        .await
        .map_err(halt("commit the owner's step"))?;
    writer
        .commit()
        .await
        .map_err(halt("release the writer's locks"))
}

#[cfg(test)]
mod tests;
