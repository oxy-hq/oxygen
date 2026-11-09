//! `workspace_periodic_ticks`: which workspaces this tick drives, claimed so
//! that each is driven by one node per interval however many nodes tick.
//!
//! Every statement uses the database's clock, never this process's: the due
//! test compares a timestamp the database wrote with the database's `now()`,
//! so nodes with skewed clocks still agree on who is due. The claim is one
//! statement — select the due rows with `FOR UPDATE SKIP LOCKED`, move each
//! one interval out, return them joined to their workspace — so two nodes
//! ticking in the same instant lock disjoint sets and neither waits on the
//! other. There is no leader: exactly-once comes from the row lock, the same
//! way `tick_schedules` gets it from the `next_run_at` compare-and-set.
//!
//! `pub` for one reason: `crates/app/tests/platform/periodic_claim.rs` runs
//! [`seed`] and [`claim`] against a real Postgres, because this SQL gates
//! every schedule the Cloud fleet fires.

use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};
use uuid::Uuid;

/// A workspace this node drives for the current interval, with what the
/// per-workspace reconcile needs to decide whether it has anything to do.
#[derive(Debug, Clone)]
pub struct Claimed {
    pub workspace_id: Uuid,
    /// `workspaces.path`; `None` means the row cannot be driven yet.
    pub path: Option<String>,
    /// `workspaces.current_revision_id` as of the claim.
    pub current_revision_id: Option<Uuid>,
    /// The revision the schedules were last reconciled from.
    pub last_reconciled_revision_id: Option<Uuid>,
    pub last_reconciled_at: Option<DateTimeWithTimeZone>,
}

fn statement(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// Give every workspace without a row one, due at a random point in the next
/// `interval_secs`. The random phase is the jitter: later ticks are one
/// interval after the claim, so workspaces stay spread across the interval
/// instead of all coming due in the same second. A workspace that already has
/// a row is left exactly as it is.
pub async fn seed(db: &DatabaseConnection, interval_secs: i64) -> Result<u64, DbErr> {
    let seeded = db
        .execute_raw(statement(
            "INSERT INTO workspace_periodic_ticks (workspace_id, next_tick_at) \
             SELECT w.id, now() + random() * $1::bigint * interval '1 second' \
             FROM workspaces w \
             ON CONFLICT (workspace_id) DO NOTHING",
            vec![interval_secs.into()],
        ))
        .await?;
    Ok(seeded.rows_affected())
}

/// Take up to `batch` due workspaces for this node, oldest due first, moving
/// each one `interval_secs` out so no other node takes it this interval.
///
/// `SKIP LOCKED` is what makes concurrent ticks cheap: a row another node is
/// claiming in the same instant is skipped rather than waited for, and that
/// node's UPDATE will have moved it out of the due set by the time this
/// statement's snapshot is re-checked. The join to `workspaces` happens in the
/// same statement so the tick needs no second read per workspace.
pub async fn claim(
    db: &DatabaseConnection,
    batch: i64,
    interval_secs: i64,
) -> Result<Vec<Claimed>, DbErr> {
    let rows = db
        .query_all_raw(statement(
            "UPDATE workspace_periodic_ticks t \
             SET next_tick_at = now() + $1::bigint * interval '1 second', \
                 claimed_at = now() \
             FROM workspaces w \
             WHERE w.id = t.workspace_id \
               AND t.workspace_id IN ( \
                   SELECT p.workspace_id FROM workspace_periodic_ticks p \
                   WHERE p.next_tick_at <= now() \
                   ORDER BY p.next_tick_at \
                   LIMIT $2 \
                   FOR UPDATE SKIP LOCKED) \
             RETURNING t.workspace_id, w.path, w.current_revision_id, \
                       t.last_reconciled_revision_id, t.last_reconciled_at",
            vec![interval_secs.into(), batch.into()],
        ))
        .await?;
    rows.iter()
        .map(|row| {
            Ok(Claimed {
                workspace_id: row.try_get("", "workspace_id")?,
                path: row.try_get("", "path")?,
                current_revision_id: row.try_get("", "current_revision_id")?,
                last_reconciled_revision_id: row.try_get("", "last_reconciled_revision_id")?,
                last_reconciled_at: row.try_get("", "last_reconciled_at")?,
            })
        })
        .collect()
}

/// How many workspaces are due right now and not yet claimed. Read once per
/// tick, after the drain has yielded, for the `oxy_periodic_due_backlog`
/// gauge: what is left is what the fleet could not drive inside the interval,
/// so a value that stays above zero across ticks means the fleet is not
/// keeping up with its workspaces. (Before the first claim it would count
/// every workspace on a healthy fleet: a claimed row comes due again one
/// interval later, exactly when the next tick starts.)
pub async fn due_backlog(db: &DatabaseConnection) -> Result<i64, DbErr> {
    let row = db
        .query_one_raw(statement(
            "SELECT count(*)::bigint AS due FROM workspace_periodic_ticks \
             WHERE next_tick_at <= now()",
            vec![],
        ))
        .await?;
    match row {
        Some(row) => row.try_get("", "due"),
        None => Ok(0),
    }
}

/// [`seed`], logging a failure. Best-effort: the claim still runs against the
/// rows that exist, and a workspace left without a row is seeded next tick.
pub(super) async fn seed_all(db: &DatabaseConnection, interval_secs: i64) {
    if let Err(e) = seed(db, interval_secs).await {
        tracing::warn!(
            target: "recovery",
            error = %e,
            "periodic tick: seeding workspace tick rows failed"
        );
    }
}

/// [`claim`], logging a failure as an empty batch: the rows stay due for the
/// next tick, on this node or another.
pub(super) async fn claim_batch(
    db: &DatabaseConnection,
    batch: i64,
    interval_secs: i64,
) -> Vec<Claimed> {
    match claim(db, batch, interval_secs).await {
        Ok(claimed) => claimed,
        Err(e) => {
            tracing::warn!(
                target: "recovery",
                error = %e,
                "periodic tick: claiming due workspaces failed; nothing more is driven this tick"
            );
            Vec::new()
        }
    }
}

/// Write down that `workspace_id`'s schedules were just reconciled from
/// `revision_id`, so the next ticks can skip the reconcile until the promoted
/// revision changes or the forced interval comes round.
pub(super) async fn record_reconciled(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    revision_id: Option<Uuid>,
) {
    let written = db
        .execute_raw(statement(
            "UPDATE workspace_periodic_ticks \
             SET last_reconciled_revision_id = $2, last_reconciled_at = now() \
             WHERE workspace_id = $1",
            vec![workspace_id.into(), revision_id.into()],
        ))
        .await;
    if let Err(e) = written {
        tracing::warn!(
            target: "recovery",
            %workspace_id,
            error = %e,
            "periodic tick: recording the schedule reconcile failed; it runs again next tick"
        );
    }
}
