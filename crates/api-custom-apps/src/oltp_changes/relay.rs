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
//! - `run`: the pass under the lease, steps 1 to 5 below.
//! - `pass`: one writer's batches.
//! - `baseline`: the store's baselines, begun, copied and completed.
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
//!    nothing shipped and drains the mark (7), so the sweep stops waking the
//!    store. A generation bump re-stamping ends it too, leaving the mark due.
//! 2. A log row older than the stream's generation logs
//!    `oltp_changes_old_generation_row`.
//! 3. **Requests.** An open baseline whose table's capture is gone is
//!    abandoned, and each installed table with no baseline in the stream's
//!    generation gets one requested (`missing`): tables captured before
//!    baselines existed, and a safety net. A failure here is recorded as
//!    `baselines: …`, and the pass ships anyway.
//! 4. Each writer with changes waiting, the longest waiting first, ships
//!    batches until it has none left, or reaches [`Limits`]; no batch starts
//!    after the pass's budget. A writer that fails is logged and passed over.
//! 5. **Views pending**, after the shipping: each current-state view an
//!    earlier render left pending (Airhouse refused it) is made again, whether
//!    or not its table shipped rows; then the **baselines**, so a failing one
//!    never stops the shipping (see below). None begins or copies after the
//!    pass's budget.
//! 6. The lease is released, on every exit after it was taken.
//! 7. The mark is settled (`settle`): a pass that failed after taking the
//!    lease backs it off ([`BACKOFF_BASE`] doubling to [`BACKOFF_CAP`]), and
//!    one with no failed writer ends any backoff. The mark is drained as of
//!    `started` only if no writer failed and none stopped with rows left (a
//!    held row is not left); with rows left and none failed, a next pass is
//!    queued now rather than at the sweep. A baseline left open, or one that
//!    wrote log rows this pass, is rows left. A pass that leaves a baseline
//!    backing off also wakes the mark after its delay (`wake_at`), counting no
//!    failure, so the baseline is retried though no write nudges the store.
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
//!    then `_progress`. Then each current-state view the batch needs, on its
//!    own, after renewing the lease again, with the latest shape's columns and
//!    the key of the shape the latest complete baseline copied, once the batch
//!    is confirmed (8). Best effort: a view Airhouse refuses, or any error of
//!    its bookkeeping, logs `oltp_changes_view_failed`; a refused one is
//!    recorded pending in the store for a later pass (5, above).
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
//!
//! # A baseline (design §5.5)
//!
//! A baseline restarts a table's current state in Airhouse from a fresh copy
//! of its rows: a `b` marker, chunks of `s` (snapshot) rows, then a `c`
//! marker, each a log row the next batches ship like any other. The pass
//! takes the store's open baselines in `id` order, a writer at a time, each
//! under the lease renewed first:
//!
//! - A held writer's or table's baseline waits, and does not keep the mark
//!   due: the hold's removal nudges.
//! - **Begin** a requested one. It may not begin while an install of its
//!   table is in progress: the install's own nudge brings the next pass.
//! - **Copy** in chunks of at most [`COPY_CHUNK_ROWS`], each only as large as
//!   keeps the writer's unshipped rows (its held tables left out) under
//!   [`COPY_BACKLOG`]. A full backlog pauses the writer's copies, left open
//!   for the next pass, which ships some first. Each chunk resumes past the
//!   last key the store recorded, so no row is copied twice.
//! - **Complete** it once a chunk finds no rows left.
//!
//! A begin and each chunk take two sessions on the store: the writer's takes
//! the lock (the table's `SHARE`, or the chunk's rows `FOR SHARE`) and the
//! owner's writes the marker or the rows and commits **first**, so they draw
//! their `seq` while the table's writers are held off; only then does the
//! writer's commit release the lock. Every transaction of a step pins its
//! search path to `pg_catalog`, so nothing the app's schema defines is
//! reached, and its statements to [`BASELINE_STATEMENT_TIMEOUT`].
//!
//! A lock that waits past [`BEGIN_LOCK_TIMEOUT`], or a chunk whose rows keep
//! changing under its reads, backs that baseline off in the store, doubling
//! from 5 seconds to 30 minutes; it waits out its backoff without failing the
//! writer, and the pass wakes the mark when it ends. So does a key whose type
//! has no default btree order. Any other error fails the writer for the pass,
//! and the mark's backoff retries it.

use std::time::Duration;

use sea_orm::DatabaseConnection;
use uuid::Uuid;

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

mod baseline;
pub mod check;
mod executor;
pub mod holds;
pub mod marks;
mod pass;
pub mod render;
mod run;
mod settle;
pub mod sink;
mod store;
pub mod sweep;
pub mod table;

pub use executor::{RelayExecutor, enqueue_relay};

use baseline::Baselines;
use pass::WriterPass;

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
/// The most rows one chunk of a baseline's copy reads: [`BATCH_ROWS`], so a
/// chunk, one source transaction, is read whole by one batch's head.
pub const COPY_CHUNK_ROWS: i64 = 1000;
/// A writer's unshipped log rows at which its copies pause:
/// `2 × BATCHES_PER_WRITER × BATCH_ROWS`, so a copy never puts more in the log
/// than two passes ship.
pub const COPY_BACKLOG: i64 = 10_000;
/// How long a baseline's begin waits for its locks, and a chunk for its rows'
/// `FOR SHARE`, before the baseline backs off.
pub const BEGIN_LOCK_TIMEOUT: Duration = Duration::from_secs(1);
/// The longest one statement of a baseline's step may run, on either session,
/// before it fails the writer for the pass.
pub const BASELINE_STATEMENT_TIMEOUT: Duration = Duration::from_secs(10);

const _: () = assert!(NUDGE_DEBOUNCE.as_secs() < REDRAIN_MARGIN.as_secs());
const _: () = assert!(PASS_BUDGET.as_secs() < LEASE_TTL.as_secs());
const _: () = assert!(SINK_TIMEOUT.as_secs() < LEASE_TTL.as_secs());
// Between two renewals a batch makes at most two Airhouse calls (the DDL's
// column read and its statements), so even both hung cannot outlast it.
const _: () = assert!(2 * SINK_TIMEOUT.as_secs() < LEASE_TTL.as_secs());
const _: () = assert!(COPY_CHUNK_ROWS <= BATCH_ROWS);
// Between two renewals a step runs at most four statements that do work (a
// chunk's read, its txid and its write, then a complete), so even all of them
// cut off at the timeout cannot outlast the lease.
const _: () = assert!(4 * BASELINE_STATEMENT_TIMEOUT.as_secs() < LEASE_TTL.as_secs());
const _: () = assert!(COPY_BACKLOG < BATCH_CEILING);

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
    /// [`COPY_CHUNK_ROWS`].
    pub copy_chunk_rows: i64,
    /// [`COPY_BACKLOG`].
    pub copy_backlog: i64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            batch_rows: BATCH_ROWS,
            batch_ceiling: BATCH_CEILING,
            batches_per_writer: BATCHES_PER_WRITER,
            budget: PASS_BUDGET,
            copy_chunk_rows: COPY_CHUNK_ROWS,
            copy_backlog: COPY_BACKLOG,
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
    /// Baselines begun: their `b` markers written to the log.
    pub begun: u64,
    /// Snapshot rows the baselines copied into the log.
    pub copied: u64,
    /// Baselines completed: their `c` markers written to the log.
    pub completed: u64,
    /// Another relay held the lease: nothing was done.
    pub skipped_lease: bool,
}

impl RelayStats {
    fn add(&mut self, writer: WriterPass) {
        self.shipped += writer.shipped;
        self.batches += writer.batches;
        self.held += writer.held;
    }

    fn add_baselines(&mut self, baselines: &Baselines) {
        self.begun += baselines.begun;
        self.copied += baselines.copied;
        self.completed += baselines.completed;
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
    let ran = run::run(db, org_id, environment, limits, &lease).await;
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

#[cfg(test)]
mod tests;
