//! The store's baselines in a pass (step 5, and "A baseline", in the module
//! docs of `relay`), each under the lease: begun, copied in paced chunks,
//! completed. The SQL is `oxy_oltp::changes::baseline`'s; this is when, and
//! in what order, it runs.
//!
//! - **Which.** Every open baseline, in `id` order, a writer at a time. A held
//!   writer's or table's waits, as the holds stand after the pass's shipping,
//!   which may have made more, and does not keep the mark due: the hold's
//!   removal nudges. One backing off (below) waits too, and keeps the mark
//!   due at its `retry_at` ([`Baselines::retry_at`]), without a failure.
//! - **The writer's session** is opened once per writer that has an open
//!   baseline neither held nor backing off, as the install opens it, before
//!   the deadline.
//! - **Its steps** (`step`): a begin and each chunk, on the owner's and the
//!   writer's sessions, each transaction pinned, in Ruling 1's order, the
//!   owner's committing first.
//! - **Outcomes.** A begin not due (an install in progress, another baseline
//!   of the table still copying) waits for the install's own nudge. A lock
//!   timeout on either session, or a chunk whose rows kept changing under it,
//!   backs that baseline off in the store (`backoff`): the writer is not
//!   failed, so a begin queued behind a long transaction does not stall the
//!   table's writes on every nudge. Any other error, a statement timeout
//!   included, fails the writer for the pass, naming the table; the writers
//!   after it go on.

use std::time::Instant;

use std::time::Duration;

use oxy_oltp::changes::{
    ABANDON_ORPHANS_SQL, BACKLOG_SQL, OPEN_BASELINES_SQL, REQUEST_MISSING_SQL,
};
use oxy_oltp::connect::pg_detail;
use oxy_oltp::schema::WriterRef;
use tokio_postgres::{Client, Row};

use super::Limits;
use super::holds::{self, Holds};
use super::pass::{Pass, Stop, renew};
use super::store::pg;

mod backoff;
mod shape;
mod step;

use self::backoff::back_off;
use self::shape::Shape;
use self::step::{Step, failed_on, settled};

/// What a pass's baselines did.
#[derive(Debug, Default)]
pub(super) struct Baselines {
    /// Baselines begun: their `b` markers written.
    pub begun: u64,
    /// Snapshot rows copied.
    pub copied: u64,
    /// Baselines completed: their `c` markers written.
    pub completed: u64,
    /// Rows are left for the next pass: a baseline was paused by its writer's
    /// backlog or left by the deadline, or this pass wrote log rows.
    pub open: bool,
    /// Each writer whose baselines failed, as `<writer>: <error>`.
    pub failed: Vec<String>,
    /// How long until the earliest open baseline backing off, backed off
    /// this pass or skipped for it, no hold keeping it waiting, is retried:
    /// the mark is woken then (`marks::wake`).
    pub wake_in: Option<Duration>,
}

impl Baselines {
    /// Wake the mark after `after` too, if sooner.
    fn wake_by(&mut self, after: Option<Duration>) {
        self.wake_in = self.wake_in.into_iter().chain(after).min();
    }
}

/// One open baseline, as [`OPEN_BASELINES_SQL`] reads it. Its cursor moves as
/// the pass copies.
struct Open {
    id: i64,
    relid: i64,
    writer: String,
    table: String,
    /// Not yet begun.
    requested: bool,
    /// The shape version it copies, once begun.
    schema_version: Option<i32>,
    /// The last key copied, as `jsonb` text: bound as text, never through a
    /// JSON value, so no number in it passes through a float.
    last_key: Option<String>,
    /// Backed off until a `retry_at` still ahead, by the store's clock.
    backing_off: bool,
    /// Seconds until `retry_at`, by the store's clock.
    retry_in: Option<f64>,
    attempts: i32,
}

impl Open {
    fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error> {
        let state: String = row.try_get("state")?;
        Ok(Self {
            id: row.try_get("id")?,
            relid: row.try_get("relid")?,
            writer: row.try_get("writer")?,
            table: row.try_get("table_name")?,
            requested: state == "requested",
            schema_version: row.try_get("schema_version")?,
            last_key: row.try_get("last_key")?,
            backing_off: row.try_get("backing_off")?,
            retry_in: row.try_get("retry_in")?,
            attempts: row.try_get("attempts")?,
        })
    }

    /// Begun under `schema_version`: it copies from the start.
    fn began(&mut self, schema_version: i32) {
        self.requested = false;
        self.schema_version = Some(schema_version);
        self.last_key = None;
    }
}

/// One writer's baselines that do not wait, in `id` order.
struct Group {
    writer: String,
    /// Its held tables, left out of its backlog.
    held: Vec<String>,
    baselines: Vec<Open>,
}

/// A writer's two sessions on the store, and what its baselines share.
struct Sessions<'s> {
    owner: &'s mut Client,
    writer: Client,
    /// The bare writer name.
    name: String,
    /// `app_<name>`: what snapshot rows are signed for.
    schema: String,
    /// Its held tables, left out of its backlog.
    held: Vec<String>,
}

/// Abandon every open baseline whose table's capture is gone. How many.
pub(super) async fn abandon_orphans(owner: &Client) -> Result<u64, String> {
    owner
        .execute(ABANDON_ORPHANS_SQL, &[])
        .await
        .map_err(pg("abandon the baselines of tables no longer captured"))
}

/// Request a baseline of each installed table with none in the stream's
/// generation: tables captured before baselines existed, and a safety net.
/// Each request made, as `(writer, table)`.
pub(super) async fn request_missing(owner: &Client) -> Result<Vec<(String, String)>, String> {
    let doing = "request the missing baselines";
    let rows = owner
        .query(REQUEST_MISSING_SQL, &[])
        .await
        .map_err(pg(doing))?;
    rows.iter()
        .map(|row| Ok((row.try_get("writer")?, row.try_get("table_name")?)))
        .collect::<Result<_, tokio_postgres::Error>>()
        .map_err(pg(doing))
}

/// Take every open baseline that does not wait, a writer at a time, while
/// the budget lasts. A writer that fails is recorded in
/// [`Baselines::failed`] and passed over; a lost lease stops them all
/// ([`Stop::Abort`]); the holds or the baselines unread fail them all
/// ([`Stop::Failed`]).
pub(super) async fn run(pass: &Pass<'_>, owner: &mut Client) -> Result<Baselines, Stop> {
    let holds = holds::read(pass.db, pass.org_id, pass.environment)
        .await
        .map_err(|e| Stop::Failed(format!("read the relay's holds: {e}")))?;
    let open = open_baselines(owner).await.map_err(Stop::Failed)?;
    let mut done = Baselines::default();
    done.wake_by(backing_off_for(&open, &holds));
    for group in groups(open, &holds) {
        if Instant::now() >= pass.deadline {
            done.open = true;
            break;
        }
        let writer = group.writer.clone();
        match run_writer(pass, owner, group, &mut done).await {
            Ok(()) => {}
            Err(stop @ Stop::Abort(_)) => return Err(stop),
            Err(Stop::Failed(error)) => {
                tracing::warn!(org_id = %pass.org_id, environment = pass.environment, database = pass.database, %writer, %error, "oltp_changes: a baseline failed; the writer's baselines retry after the backoff");
                done.failed.push(format!("{writer}: {error}"));
            }
        }
    }
    Ok(done)
}

/// Every requested or copying baseline, in `id` order.
async fn open_baselines(owner: &Client) -> Result<Vec<Open>, String> {
    let doing = "read the open baselines";
    let rows = owner
        .query(OPEN_BASELINES_SQL, &[])
        .await
        .map_err(pg(doing))?;
    rows.iter()
        .map(Open::from_row)
        .collect::<Result<_, _>>()
        .map_err(pg(doing))
}

/// Whether a hold keeps `b` waiting: its writer's, or its table's.
fn held(holds: &Holds, b: &Open) -> bool {
    holds.writer(&b.writer) || holds.tables(&b.writer).contains(&b.table)
}

/// How long until the earliest of `open`'s baselines backing off that no
/// hold keeps waiting is retried: the hold's removal nudges those.
fn backing_off_for(open: &[Open], holds: &Holds) -> Option<Duration> {
    open.iter()
        .filter(|b| b.backing_off && !held(holds, b))
        .filter_map(|b| Duration::try_from_secs_f64(b.retry_in?.max(0.0)).ok())
        .min()
}

/// `open` grouped by writer, each writer in the order of its first baseline;
/// a held writer's or a held table's left out, to wait, and one backing off.
fn groups(open: Vec<Open>, holds: &Holds) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for b in open {
        if held(holds, &b) || b.backing_off {
            continue;
        }
        match groups.iter_mut().find(|g| g.writer == b.writer) {
            Some(group) => group.baselines.push(b),
            None => groups.push(Group {
                writer: b.writer.clone(),
                held: holds.tables(&b.writer),
                baselines: vec![b],
            }),
        }
    }
    groups
}

/// One writer's baselines, in `id` order, on a session of its own: each
/// begun if requested, then copied, until the writer's copies stop for the
/// pass.
async fn run_writer(
    pass: &Pass<'_>,
    owner: &mut Client,
    group: Group,
    done: &mut Baselines,
) -> Result<(), Stop> {
    let (writer, schema) = writer_session(pass, &group.writer)
        .await
        .map_err(Stop::Failed)?;
    let mut s = Sessions {
        owner,
        writer,
        name: group.writer,
        schema,
        held: group.held,
    };
    for mut b in group.baselines {
        if Instant::now() >= pass.deadline {
            done.open = true;
            return Ok(());
        }
        renew(pass).await?;
        if !begun(pass, &mut s, &mut b, done).await? {
            continue;
        }
        if !copy(pass, &mut s, &mut b, done).await? {
            return Ok(());
        }
    }
    Ok(())
}

/// Begin `b` if it is requested. Whether it is copying now: not when its
/// begin is not due yet, or backed it off.
async fn begun(
    pass: &Pass<'_>,
    s: &mut Sessions<'_>,
    b: &mut Open,
    done: &mut Baselines,
) -> Result<bool, Stop> {
    if !b.requested {
        return Ok(true);
    }
    let outcome = step::begin(s, b).await;
    match settled(&b.table, outcome)? {
        Step::Began(schema_version) => {
            done.begun += 1;
            done.open = true;
            b.began(schema_version);
            Ok(true)
        }
        Step::NotYet => Ok(false),
        step @ (Step::LockTimeout | Step::Conflicted) => {
            done.wake_by(back_off(pass, s, b, step).await?);
            Ok(false)
        }
        // A begin copies, completes and abandons nothing, and reads no key.
        Step::Copied(_) | Step::Completed | Step::Abandoned | Step::Unordered => Ok(false),
    }
}

/// A session on the store as `writer`'s role, opened as the install opens
/// it, and `writer`'s schema.
async fn writer_session(pass: &Pass<'_>, writer: &str) -> Result<(Client, String), String> {
    let writer =
        WriterRef::app(writer.to_owned()).map_err(|e| format!("the writer's schema: {e}"))?;
    let conn = oxy_oltp::resolver::resolve_writer_connection_for_org(pass.db, pass.org_id, &writer)
        .await
        .map_err(|e| format!("resolve the app's writer: {e}"))?;
    let client = oxy_oltp::connect::connect(&conn.dsn, "oxy_changes baseline")
        .await
        .map_err(|e| {
            format!(
                "open the org's OLTP store as the app's writer: {}",
                pg_detail(&e)
            )
        })?;
    Ok((client, writer.schema_name()))
}

/// Copy `b` from its cursor in paced chunks. `true` when it completed, was
/// abandoned or backed off, and the writer's next baseline goes on; `false`
/// when the writer's backlog is full or the deadline has passed, which leaves
/// it open and stops the writer's copies for the pass.
async fn copy(
    pass: &Pass<'_>,
    s: &mut Sessions<'_>,
    b: &mut Open,
    done: &mut Baselines,
) -> Result<bool, Stop> {
    let shape = shape::read(s, b)
        .await
        .map_err(|e| failed_on(&b.table, e))?;
    if !shape.ordered() {
        done.wake_by(back_off(pass, s, b, Step::Unordered).await?);
        return Ok(true);
    }
    loop {
        let rows = if Instant::now() < pass.deadline {
            let backlog = backlog(pass, s).await.map_err(|e| failed_on(&b.table, e))?;
            pace(backlog, pass.limits)
        } else {
            0
        };
        if rows == 0 {
            done.open = true;
            return Ok(false);
        }
        renew(pass).await?;
        let outcome = step::chunk(pass, s, b, &shape, rows).await;
        match settled(&b.table, outcome)? {
            Step::Copied(rows) => {
                done.copied += rows;
                done.open = true;
            }
            Step::Completed => {
                done.completed += 1;
                done.open = true;
                return Ok(true);
            }
            Step::Abandoned => return Ok(true),
            step @ (Step::LockTimeout | Step::Conflicted) => {
                done.wake_by(back_off(pass, s, b, step).await?);
                return Ok(true);
            }
            // A chunk never begins a baseline, and its key was read.
            Step::Began(_) | Step::NotYet | Step::Unordered => return Ok(true),
        }
    }
}

/// How many of the writer's log rows wait to ship, its held tables left out,
/// counted no further than the backlog that pauses its copies.
async fn backlog(pass: &Pass<'_>, s: &Sessions<'_>) -> Result<i64, String> {
    let doing = "count the writer's unshipped rows";
    let row = s
        .owner
        .query_one(
            BACKLOG_SQL,
            &[
                &pass.generation,
                &s.name,
                &s.held,
                &pass.limits.copy_backlog,
            ],
        )
        .await
        .map_err(pg(doing))?;
    row.try_get(0).map_err(pg(doing))
}

/// How many rows the next chunk may copy: as many as keep the writer's
/// `backlog` under `limits.copy_backlog`, at most `limits.copy_chunk_rows`;
/// none once the backlog is full.
fn pace(backlog: i64, limits: &Limits) -> i64 {
    limits
        .copy_backlog
        .saturating_sub(backlog)
        .clamp(0, limits.copy_chunk_rows.max(0))
}

#[cfg(test)]
mod tests;
