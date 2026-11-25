//! The relay (OLTP change stream design §5.3 and §5.4): the rows a store's
//! `oxy_changes.log` captured, shipped into typed, append-only change tables
//! in the app's workspace Airhouse, one schema per writer (`oltp_<writer>`),
//! each change exactly once.
//!
//! - [`table`]: a declared table's change table, planned from every shape its
//!   rows were captured under: the Postgres columns, typed for DuckDB, which
//!   only ever gain columns.
//! - [`marks`]: the platform's nudge and drain marks and the single-writer lease.
//! - [`holds`]: the tables or writers the relay must stop shipping, with a reason.
//! - [`render`]: that plan as SQL: the schema, the change tables and their
//!   `ALTER`s, each batch's idempotent `INSERT`, and `_progress`.
//! - [`sink`]: the workspace Airhouse, behind a trait tests replace.
//! - [`check`]: the restore check, run before every batch.
//! - `store`: the store's side of a pass, as its owner.
//! - `pass`: one writer's batches.
//! - `settle`: the end of a pass: the mark's backoff, its drain, the next pass.
//! - `executor`: the pass as a queued task ([`enqueue_relay`], [`RelayExecutor`]).
//!
//! # One pass ([`relay_once`])
//!
//! Production only, with `oltp_changes` on for the org: any other
//! environment, or the flag off, is a no-op that opens no store. The pass
//! takes the (org, environment) lease, or does nothing
//! ([`RelayStats::skipped_lease`]). Then:
//!
//! 1. `started` from the platform's clock, which the marks are on. With no
//!    store that captures (none, inactive, or below step 8), or capture never
//!    installed (no `stream` row, and so no log row), the pass ends here with
//!    nothing shipped and drains the mark (5), so the sweep stops waking the
//!    store. A generation bump re-stamping ends it too, leaving the mark due.
//! 2. A log row older than the stream's generation logs
//!    `oltp_changes_old_generation_row`.
//! 3. Each writer with changes waiting, the longest waiting first, ships
//!    batches until it has none left, or reaches [`Limits`]; no batch starts
//!    after the pass's budget. A writer that fails is logged and passed over.
//! 4. The lease is released, on every exit after it was taken.
//! 5. The mark is settled (`settle`): a pass that failed after taking the
//!    lease backs it off ([`BACKOFF_BASE`] doubling to [`BACKOFF_CAP`]), and
//!    one with no failed writer ends any backoff. The mark is drained as of
//!    `started` only if no writer failed and none stopped with rows left (a
//!    held row is not left); with rows left and none failed, a next pass is
//!    queued now rather than at the sweep.
//!
//! # One batch of a writer
//!
//! 1. **Holds.** A held writer ships nothing, and its held tables are not read.
//! 2. **App and Airhouse.** The org's app whose writer it is, and its
//!    workspace's Airhouse; without either, the writer is held for that.
//! 3. **Restore check** ([`check`]), before any read or DDL, under the lease
//!    renewed at the batch's start (a lost lease stops the pass): Airhouse's
//!    `_progress` against the store's `oxy_changes.progress`. A restore holds
//!    the writer until a generation bump (PR 3) and logs
//!    `oltp_changes_restore_detected`.
//! 4. **Read and declare**, in one store transaction: the batch, whole source
//!    transactions, and its number as the writer's intent. Nothing waiting
//!    ends the writer. A batch never passes [`BATCH_CEILING`] rows: several
//!    transactions that would are cut back to the oldest alone, and one that
//!    passes it by itself holds the writer (`oltp_changes_held`), with
//!    nothing declared.
//! 5. **Plan**, table by table. A table that cannot ship is held, its rows
//!    left in the log; the others ship. Each row's MAC decides its actor. A
//!    batch whose oldest change waited past [`STALE_AFTER`] logs
//!    `oltp_changes_stale`, once per writer.
//! 6. **DDL**, as Admin, after renewing the lease: the schema, `_progress`,
//!    and each change table made or given its new columns.
//! 7. **Write**, after renewing the lease again: one DuckLake transaction of
//!    every table's idempotent `INSERT`s, at most [`INSERT_ROWS`] rows each,
//!    then `_progress`.
//! 8. **Confirm and delete**, in one store transaction: the batch recorded
//!    shipped only if it is still the intent, then exactly its `seq`s
//!    deleted. An intent that moved means the store was rewound under the
//!    relay: the writer stops, and the next pass's check finds it.
//!
//! A pass that dies between 7 and 8 leaves Airhouse between the store's
//! shipped batch and its intent: the next pass ships the same rows again
//! under a new number, and the anti-join adds none twice.
//!
//! Every Airhouse call is bounded by [`SINK_TIMEOUT`], well inside
//! [`LEASE_TTL`]: a hung call fails its writer rather than outlast the lease.
//! A write that times out after its `COMMIT` was sent is the pass that died
//! between 7 and 8.

use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use sea_orm::DatabaseConnection;
use tokio_postgres::Client;
use uuid::Uuid;

use crate::app_events::marks::platform_now;

// `error` in production, `warn` elsewhere: how each of the relay's alarm
// lines is logged. Every pass is production in phase 1.
macro_rules! alarm {
    ($environment:expr, $($field:tt)+) => {
        if $environment == $crate::oltp_changes::relay::PRODUCTION {
            tracing::error!($($field)+)
        } else {
            tracing::warn!($($field)+)
        }
    };
}

pub mod check;
mod executor;
pub mod holds;
pub mod marks;
mod pass;
pub mod render;
mod settle;
pub mod sink;
mod store;
pub mod sweep;
pub mod table;

pub use executor::{RelayExecutor, enqueue_relay};

use pass::{Pass, Stop, WriterPass};
use settle::Ran;
use store::Waiting;

/// The `TaskSpec::Custom` kind, and the run's `source_type`.
pub const RELAY_KIND: &str = "oltp_changes_relay";

/// The environment the relay ships, and the one every MAC is verified for.
const PRODUCTION: &str = "production";

/// How long a process waits before nudging the relay again for the same
/// (org, environment).
pub const NUDGE_DEBOUNCE: Duration = Duration::from_secs(5);

/// How long after a nudge the relay re-drains what it may have missed.
///
/// [`NUDGE_DEBOUNCE`] must stay shorter: a commit the debounce suppressed
/// lands inside this margin of the nudge that suppressed it, so the re-drain
/// still sees it.
pub const REDRAIN_MARGIN: Duration = Duration::from_secs(30);

/// How often the fleet sweep looks at the platform's marks.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(5);

/// How long after its last drain began a mark is due again, for the sweep.
pub const REDRAIN_EVERY: Duration = Duration::from_secs(10);

/// How often the weekly sweep nudges every mark, so each store is drained
/// whatever its marks say.
pub const WEEKLY_INTERVAL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The rows a batch reads before it extends to the end of its last source
/// transaction.
pub const BATCH_ROWS: i64 = 1000;
/// The most rows one batch ships. A source transaction larger than this is
/// never split: its writer is held, until a resync (PR 3).
pub const BATCH_CEILING: i64 = 50_000;
/// The most rows one `INSERT` of a batch carries: a table's rows are sent as
/// several, all in the batch's one transaction.
pub const INSERT_ROWS: usize = 500;
/// The most batches one writer ships in a pass.
pub const BATCHES_PER_WRITER: usize = 5;
/// How long a pass starts batches for.
pub const PASS_BUDGET: Duration = Duration::from_secs(60);
/// How long the lease lasts unless renewed; renewed at the start of each
/// batch, before its DDL, and before its write.
pub const LEASE_TTL: Duration = Duration::from_secs(120);
/// The longest one Airhouse call may take ([`sink::Bounded`]) before it fails
/// its writer for the pass.
pub const SINK_TIMEOUT: Duration = Duration::from_secs(45);
/// How long a change may wait before the relay logs `oltp_changes_stale`.
pub const STALE_AFTER: Duration = Duration::from_secs(5 * 60);
/// How long the sweep leaves a mark after its first failed pass; doubled
/// for each failure after, up to [`BACKOFF_CAP`].
pub const BACKOFF_BASE: Duration = Duration::from_secs(5);
/// The longest the sweep leaves a failing mark.
pub const BACKOFF_CAP: Duration = Duration::from_secs(30 * 60);

const _: () = assert!(NUDGE_DEBOUNCE.as_secs() < REDRAIN_MARGIN.as_secs());
const _: () = assert!(PASS_BUDGET.as_secs() < LEASE_TTL.as_secs());
const _: () = assert!(SINK_TIMEOUT.as_secs() < LEASE_TTL.as_secs());
// Between two renewals a batch makes at most two Airhouse calls (the DDL's
// column read and its statements), so even both hung cannot outlast it.
const _: () = assert!(2 * SINK_TIMEOUT.as_secs() < LEASE_TTL.as_secs());

/// How much one pass ships.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// [`BATCH_ROWS`].
    pub batch_rows: i64,
    /// [`BATCH_CEILING`].
    pub batch_ceiling: i64,
    /// [`BATCHES_PER_WRITER`].
    pub batches_per_writer: usize,
    /// [`PASS_BUDGET`].
    pub budget: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            batch_rows: BATCH_ROWS,
            batch_ceiling: BATCH_CEILING,
            batches_per_writer: BATCHES_PER_WRITER,
            budget: PASS_BUDGET,
        }
    }
}

/// What one pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RelayStats {
    /// Log rows shipped to Airhouse and deleted from the log.
    pub shipped: u64,
    /// Batches committed to Airhouse and confirmed.
    pub batches: u64,
    /// Rows left in the log behind a hold the pass found or made: a held
    /// writer's waiting rows, and a held table's rows in a batch it read.
    pub held: u64,
    /// Another relay held the lease: nothing was done.
    pub skipped_lease: bool,
}

impl RelayStats {
    fn add(&mut self, writer: WriterPass) {
        self.shipped += writer.shipped;
        self.batches += writer.batches;
        self.held += writer.held;
    }
}

/// One pass over `org_id`'s `environment` change stream, with the default
/// [`Limits`]. See the module docs.
pub async fn relay_once(
    db: &DatabaseConnection,
    org_id: Uuid,
    environment: &str,
) -> Result<RelayStats, String> {
    relay_once_with(db, org_id, environment, &Limits::default()).await
}

/// [`relay_once`] under `limits`. `Err` names every writer that failed, or
/// the step that stopped the pass; the mark is then left due, backed off
/// ([`BACKOFF_BASE`]).
pub async fn relay_once_with(
    db: &DatabaseConnection,
    org_id: Uuid,
    environment: &str,
    limits: &Limits,
) -> Result<RelayStats, String> {
    if environment != PRODUCTION || !super::enabled_for(org_id) {
        tracing::debug!(%org_id, environment, "oltp_changes: not relayed here; nothing to do");
        return Ok(RelayStats::default());
    }
    let holder = format!("{RELAY_KIND}:{}", Uuid::new_v4());
    let lease = marks::acquire(db, org_id, environment, &holder, LEASE_TTL)
        .await
        .map_err(|e| format!("take the relay lease: {e}"))?;
    let Some(lease) = lease else {
        return Ok(RelayStats {
            skipped_lease: true,
            ..RelayStats::default()
        });
    };
    let ran = run(db, org_id, environment, limits, &lease).await;
    if let Err(e) = marks::release(db, &lease).await {
        tracing::warn!(%org_id, environment, error = %e, "oltp_changes: could not release the relay lease");
    }
    settle::settle(db, org_id, environment, &ran).await;
    let Some(ran) = ran? else {
        return Ok(RelayStats::default());
    };
    if !ran.failed.is_empty() {
        return Err(format!(
            "relaying the change stream failed for {}",
            ran.failed.join("; ")
        ));
    }
    Ok(ran.stats)
}

/// The pass under the lease. `None` for the one exit that leaves the mark as
/// it is: a bump re-stamping (PR 3), which a later pass must still drain.
/// With no store that captures (none, inactive, or below step 8) or no
/// `stream` row, nothing can be waiting, since no log row exists without a
/// stream row: the pass is a drain that shipped nothing, so the sweep stops
/// waking the store.
async fn run(
    db: &DatabaseConnection,
    org_id: Uuid,
    environment: &str,
    limits: &Limits,
    lease: &marks::Lease,
) -> Result<Option<Ran>, String> {
    let deadline = Instant::now() + limits.budget;
    let started = platform_now(db)
        .await
        .map_err(|e| format!("read the platform clock: {e}"))?;
    let Some(mut owner) = store::open(db, org_id).await? else {
        tracing::debug!(%org_id, "oltp_changes: the org has no store that captures; nothing to relay");
        return Ok(Some(Ran::nothing(started)));
    };
    let Some(stream) = store::stream(&owner).await? else {
        tracing::debug!(%org_id, "oltp_changes: capture was never installed; nothing to relay");
        return Ok(Some(Ran::nothing(started)));
    };
    if let Some(from) = stream.restamping {
        tracing::info!(%org_id, from, to = stream.generation, "oltp_changes: a generation bump is re-stamping the log; nothing relayed");
        return Ok(None);
    }
    let older = store::older_generation_rows(&owner, stream.generation).await?;
    if older > 0 {
        alarm!(environment, %org_id, environment, rows = older, generation = stream.generation, "oltp_changes_old_generation_row");
    }
    let waiting = store::waiting(&owner, stream.generation).await?;
    let holds = holds::read(db, org_id, environment)
        .await
        .map_err(|e| format!("read the relay's holds: {e}"))?;
    let sink = sink::Bounded::new(sink::sink(), SINK_TIMEOUT);
    let pass = Pass {
        db,
        org_id,
        environment,
        lease,
        limits,
        deadline,
        generation: stream.generation,
        holds: &holds,
        sink: &sink,
    };
    ship_all(&pass, &mut owner, &waiting, started)
        .await
        .map(Some)
}

/// Step 3 of a pass: each waiting writer's batches, while the budget lasts.
/// A lost lease stops them all.
async fn ship_all(
    pass: &Pass<'_>,
    owner: &mut Client,
    waiting: &[Waiting],
    started: DateTime<Utc>,
) -> Result<Ran, String> {
    let mut ran = Ran::nothing(started);
    for writer in waiting {
        if Instant::now() >= pass.deadline {
            ran.left = true;
            break;
        }
        match pass::ship_writer(pass, owner, writer).await {
            Ok(one) => {
                ran.stats.add(one);
                ran.left |= one.left;
            }
            Err(Stop::Failed(error)) => {
                tracing::warn!(org_id = %pass.org_id, environment = pass.environment, writer = %writer.writer, %error, "oltp_changes: could not relay one writer; relaying the others");
                ran.failed.push(format!("{}: {error}", writer.writer));
            }
            Err(Stop::Abort(error)) => return Err(error),
        }
    }
    Ok(ran)
}

#[cfg(test)]
mod tests;
