//! One writer's batches, each through steps 1 to 8 of the module docs of
//! `relay`, until the writer has nothing left, is held, fails, bumps the
//! store's generation, or reaches the pass's [`Limits`](super::Limits).
//!
//! - `target`: the writer's app and its workspace Airhouse (step 2).
//! - `plan`: a batch's rows planned table by table (step 5).
//! - `airhouse`: the batch's Airhouse side (steps 3, 6 and 7).
//! - `views`: the current-state views made after a batch's write, and those
//!   an earlier render left pending.

use std::ops::ControlFlow;
use std::time::Instant;

use chrono::Utc;
use oxy_oltp::changes::LogRow;
use sea_orm::DatabaseConnection;
use tokio_postgres::Client;
use uuid::Uuid;

use super::bump::{self, Bumped};
use super::check::{StoreProgress, Verdict, check};
use super::holds::{self, HoldKey, HoldKind, Holds, Release};
use super::marks::{self, Lease};
use super::sink::ChangeSink;
use super::store::{self, Next, Read, Waiting};
use super::{LEASE_TTL, Limits, STALE_AFTER};
use crate::oltp_changes::provenance::{self, ChangeSigner};

mod airhouse;
mod plan;
mod target;
mod views;

pub(super) use self::views::rerender_pending;

use self::plan::{Ready, Refused};
use self::target::Target;

/// What every writer of one pass shares.
pub(super) struct Pass<'a> {
    pub db: &'a DatabaseConnection,
    pub org_id: Uuid,
    pub environment: &'a str,
    /// The store's database, which alarms name.
    pub database: &'a str,
    pub lease: &'a Lease,
    pub limits: &'a Limits,
    /// When the pass's budget runs out: no batch starts after it.
    pub deadline: Instant,
    /// The stream's generation, read once per pass.
    pub generation: i64,
    pub holds: &'a Holds,
    pub sink: &'a dyn ChangeSink,
}

/// Why a writer stopped short.
#[derive(Debug)]
pub(super) enum Stop {
    /// The lease was lost, or could not be renewed: the whole pass stops.
    Abort(String),
    /// This writer failed this pass; the others go on.
    Failed(String),
}

/// What one writer's batches did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct WriterPass {
    /// Log rows shipped and deleted.
    pub shipped: u64,
    /// Batches committed to Airhouse and confirmed.
    pub batches: u64,
    /// Rows left in the log behind a hold this pass found or made.
    pub held: u64,
    /// Whether rows may still wait, not counting held ones.
    pub left: bool,
    /// The restore check found the store restored, and bumped its
    /// generation: the pass ships nothing more.
    pub bumped: bool,
}

/// How one batch ended.
enum Batch {
    /// Nothing waits outside the held tables.
    Empty,
    /// Read, then shipped (`shipped` rows, possibly none) and held
    /// (`held` rows). `full`: it took the whole limit, so more may wait.
    Done { shipped: u64, held: u64, full: bool },
    /// The writer was held: its oldest transaction is past the ceiling.
    Held,
    /// The restore check found the store restored, and bumped its generation.
    Bumped,
    /// The intent moved under the batch: the store was rewound, or another
    /// relay took the lapsed lease.
    Rewound,
}

/// The writer holds a pass checks again: a restore (batch step 3) and a
/// missing target (step 2). Each is lifted once its check passes. The other
/// writer hold, the ceiling's, waits for a resync.
const CHECKED_AGAIN: [HoldKind; 2] = [HoldKind::Restore, HoldKind::Target];

/// Ship `waiting`'s batches. A held writer ships nothing, unless what held
/// it is checked again ([`CHECKED_AGAIN`]) and now passes. Nor does one with
/// no app or no Airhouse, which is held for that.
pub(super) async fn ship_writer(
    pass: &Pass<'_>,
    owner: &mut Client,
    waiting: &Waiting,
) -> Result<WriterPass, Stop> {
    let all_held = WriterPass {
        held: waiting.rows.max(0) as u64,
        ..WriterPass::default()
    };
    let kind = pass.holds.writer_kind(&waiting.writer);
    if kind.is_some_and(|kind| !CHECKED_AGAIN.contains(&kind)) {
        return Ok(all_held);
    }
    let Some(mut target) = target::find(pass, &waiting.writer).await? else {
        return Ok(all_held);
    };
    let mut done = WriterPass {
        left: true,
        ..WriterPass::default()
    };
    for _ in 0..pass.limits.batches_per_writer {
        if Instant::now() >= pass.deadline {
            break;
        }
        match ship_batch(pass, owner, &mut target).await? {
            Batch::Empty => {
                return Ok(WriterPass {
                    left: false,
                    ..done
                });
            }
            Batch::Held => {
                let held = all_held.held.saturating_sub(done.shipped);
                return Ok(WriterPass {
                    held: done.held + held,
                    left: false,
                    ..done
                });
            }
            Batch::Bumped => {
                return Ok(WriterPass {
                    bumped: true,
                    left: true,
                    ..done
                });
            }
            Batch::Rewound => return Ok(done),
            Batch::Done {
                shipped,
                held,
                full,
            } => {
                done.shipped += shipped;
                done.batches += u64::from(shipped > 0);
                done.held += held;
                done.left = full;
                if !full {
                    return Ok(done);
                }
            }
        }
    }
    Ok(done)
}

/// Steps 3 to 8 for one batch, under a lease renewed first: a relay whose
/// lease lapsed during its last write stops here, before its restore check
/// or its read could act on a stream another relay now holds.
async fn ship_batch(
    pass: &Pass<'_>,
    owner: &mut Client,
    target: &mut Target,
) -> Result<Batch, Stop> {
    renew(pass).await?;
    if restored(pass, owner, target).await? {
        return Ok(Batch::Bumped);
    }
    let read = match read_next(pass, owner, target).await? {
        ControlFlow::Continue(read) => read,
        ControlFlow::Break(ended) => return Ok(ended),
    };
    let full = read.full;
    log_stale(pass, target, &read.rows);
    let shapes = store::shapes(owner, &target.name)
        .await
        .map_err(Stop::Failed)?;
    let org_id = pass.org_id;
    let source = plan::Source {
        writer: &target.name,
        schema: &target.schema,
        generation: pass.generation,
    };
    let planned = plan::plan(
        source,
        read.rows,
        &shapes,
        &|s: &ChangeSigner<'_>, mac: Option<&[u8]>| provenance::verify(org_id, s, mac),
    );
    let held = hold_refused(pass, target, &planned.refused).await?;
    if planned.ready.is_empty() {
        return Ok(Batch::Done {
            shipped: 0,
            held,
            full,
        });
    }
    let views = write(pass, target, read.batch, &planned.ready).await?;
    let seqs: Vec<i64> = planned
        .ready
        .iter()
        .flat_map(|table| table.seqs.iter().copied())
        .collect();
    let tables: Vec<String> = views.iter().map(|v| v.table.clone()).collect();
    let confirming = store::Confirming {
        batch: read.batch,
        generation: pass.generation,
        seqs: &seqs,
        views: &tables,
    };
    if !confirm(pass, owner, target, &confirming).await? {
        return Ok(Batch::Rewound);
    }
    views::after_confirm(pass, owner, target, &views).await?;
    let shipped = seqs.len() as u64;
    Ok(Batch::Done {
        shipped,
        held,
        full,
    })
}

/// Step 3: whether the restore check found the store restored, and bumped
/// its generation ([`bump::bump`]). A writer held for a restore whose check
/// now proceeds has its hold lifted.
async fn restored(pass: &Pass<'_>, owner: &mut Client, target: &Target) -> Result<bool, Stop> {
    let (intent, shipped) = store::progress(owner, &target.name)
        .await
        .map_err(Stop::Failed)?;
    let airhouse = airhouse::shipped(pass.sink, target)
        .await
        .map_err(Stop::Failed)?;
    let store_side = StoreProgress {
        generation: pass.generation,
        intent,
        shipped,
    };
    let verdict = check(airhouse, store_side);
    if verdict == Verdict::Proceed {
        let lifted = "a writer held for a restore now passes its check; its hold is lifted";
        lift(pass, &target.name, HoldKind::Restore, lifted).await?;
        return Ok(false);
    }
    let Bumped {
        from,
        to,
        requested,
    } = bump::bump(pass, owner, airhouse).await?;
    alarm!(
        pass.environment,
        org_id = %pass.org_id,
        environment = pass.environment,
        writer = %target.name,
        ?verdict,
        ?airhouse,
        store = ?store_side,
        bumped_from = from,
        bumped_to = to,
        baselines_requested = requested,
        "oltp_changes_restore_detected"
    );
    Ok(true)
}

/// Lift `writer`'s hold of `kind`, when the pass found it held for that and
/// its check ([`CHECKED_AGAIN`]) now passes; `lifted` is logged when a hold
/// was removed.
async fn lift(pass: &Pass<'_>, writer: &str, kind: HoldKind, lifted: &str) -> Result<(), Stop> {
    if pass.holds.writer_kind(writer) != Some(kind) {
        return Ok(());
    }
    let release = Release {
        org_id: pass.org_id,
        environment: pass.environment,
        writer: Some(writer),
        tables: None,
        kinds: &[kind],
    };
    let removed = holds::release(pass.db, release)
        .await
        .map_err(|e| Stop::Failed(format!("lift the writer's {} hold: {e}", kind.as_str())))?;
    if !removed.is_empty() {
        tracing::info!(org_id = %pass.org_id, environment = pass.environment, writer, "oltp_changes: {lifted}");
    }
    Ok(())
}

/// Step 4: the writer's next batch, read and declared. Breaks with how the
/// writer's batches end when nothing waits, or when its oldest source
/// transaction alone is past the relay's ceiling: that one is never split,
/// so the writer is held, with nothing declared.
async fn read_next(
    pass: &Pass<'_>,
    owner: &mut Client,
    target: &Target,
) -> Result<ControlFlow<Batch, Read>, Stop> {
    let source = store::Source {
        generation: pass.generation,
        writer: &target.name,
        held: &target.held,
    };
    let next = store::read_and_declare(owner, &source, pass.limits)
        .await
        .map_err(Stop::Failed)?;
    match next {
        Next::Read(read) => Ok(ControlFlow::Continue(read)),
        Next::Empty => Ok(ControlFlow::Break(Batch::Empty)),
        Next::Over { rows } => {
            let ceiling = pass.limits.batch_ceiling;
            let reason = format!(
                "a transaction of {rows} rows exceeds the relay's ceiling of {ceiling} \
                 (resync after PR 3)"
            );
            hold(pass, &target.name, None, HoldKind::Ceiling, &reason).await?;
            Ok(ControlFlow::Break(Batch::Held))
        }
    }
}

/// `oltp_changes_stale`, once per writer and pass, when the batch's oldest
/// change has waited past [`STALE_AFTER`].
fn log_stale(pass: &Pass<'_>, target: &mut Target, rows: &[LogRow]) {
    let Some(oldest) = rows.iter().map(|row| row.changed_at).min() else {
        return;
    };
    let waited = (Utc::now() - oldest).to_std().unwrap_or_default();
    if target.stale_logged || waited <= STALE_AFTER {
        return;
    }
    target.stale_logged = true;
    alarm!(
        pass.environment,
        org_id = %pass.org_id,
        environment = pass.environment,
        writer = %target.name,
        waited_seconds = waited.as_secs(),
        "oltp_changes_stale"
    );
}

/// Hold each refused table, and leave it out of the writer's next read
/// this pass. How many rows they held back.
async fn hold_refused(
    pass: &Pass<'_>,
    target: &mut Target,
    refused: &[Refused],
) -> Result<u64, Stop> {
    let mut held = 0;
    for table in refused {
        target.held.push(table.table.clone());
        hold(
            pass,
            &target.name,
            Some(table.table.as_str()),
            HoldKind::Table,
            &table.reason,
        )
        .await?;
        held += table.rows as u64;
    }
    Ok(held)
}

/// Hold `table` of `writer` (the whole writer when `None`), logging
/// `oltp_changes_held` when the hold is new.
async fn hold(
    pass: &Pass<'_>,
    writer: &str,
    table: Option<&str>,
    kind: HoldKind,
    reason: &str,
) -> Result<(), Stop> {
    let key = HoldKey {
        org_id: pass.org_id,
        environment: pass.environment,
        writer,
        table,
        kind,
    };
    let new = holds::hold(pass.db, key, reason)
        .await
        .map_err(|e| Stop::Failed(format!("record a hold: {e}")))?;
    if new {
        alarm!(
            pass.environment,
            org_id = %pass.org_id,
            environment = pass.environment,
            writer,
            table = table.unwrap_or(""),
            reason,
            "oltp_changes_held"
        );
    }
    Ok(())
}

/// Steps 6 and 7, each after renewing the lease: the DDL as Admin, then the
/// batch's one DuckLake transaction as Writer. Returns the current-state
/// views the batch needs, made only once it is confirmed (`views`). Each
/// Airhouse call is bounded ([`SINK_TIMEOUT`](super::SINK_TIMEOUT)), so none
/// outlasts the lease.
async fn write(
    pass: &Pass<'_>,
    target: &Target,
    batch: i64,
    ready: &[Ready],
) -> Result<Vec<airhouse::PendingView>, Stop> {
    renew(pass).await?;
    let views = airhouse::ddl(pass.sink, target, ready)
        .await
        .map_err(Stop::Failed)?;
    renew(pass).await?;
    airhouse::write(pass.sink, target, pass.generation, batch, ready)
        .await
        .map_err(Stop::Failed)?;
    Ok(views)
}

/// Renew the pass's lease: a lost one, or one that cannot be renewed, stops
/// the whole pass.
pub(super) async fn renew(pass: &Pass<'_>) -> Result<(), Stop> {
    match marks::renew(pass.db, pass.lease, LEASE_TTL).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(Stop::Abort(
            "the relay lease was lost to another relay".into(),
        )),
        Err(e) => Err(Stop::Abort(format!("renew the relay lease: {e}"))),
    }
}

/// Step 8: confirm the batch, delete its `seqs`, and flag the views it
/// needs pending. `false` when the writer's intent is no longer the batch:
/// the store was rewound under the relay, or the lease lapsed and another
/// relay declared its own batch. Nothing is deleted, and the next pass's
/// restore check decides. Flags that fail are logged, never fail the batch.
async fn confirm(
    pass: &Pass<'_>,
    owner: &mut Client,
    target: &Target,
    confirming: &store::Confirming<'_>,
) -> Result<bool, Stop> {
    let batch = confirming.batch;
    let confirmed = store::confirm_and_delete(owner, &target.name, confirming)
        .await
        .map_err(Stop::Failed)?;
    if let Some(Err(error)) = &confirmed {
        alarm!(
            pass.environment,
            org_id = %pass.org_id,
            environment = pass.environment,
            writer = %target.name,
            %error,
            "oltp_changes_view_failed"
        );
    }
    let confirmed = confirmed.is_some();
    if !confirmed {
        tracing::warn!(
            org_id = %pass.org_id,
            environment = pass.environment,
            writer = %target.name,
            batch,
            "oltp_changes: the writer's intent moved under its batch: the store was rewound, \
             or this relay's lease lapsed and another relay declared its own batch; nothing \
             was deleted, and the next pass's restore check decides"
        );
    }
    Ok(confirmed)
}
