//! Executing a transition: recording its flips (the `experiment_flips` row,
//! unique on run, unit, state, due day — the idempotency key for the ROW) and
//! its other actions, and reading lever outcomes back. Sending the recorded
//! flips — the stamp that is the idempotency key for the JOB — is `send.rs`.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use entity::experiment_runs;
use oxy_experiment::ExperimentConfig;
use oxy_experiment::run::{LeverState, RunAction, Transition};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, FromQueryResult, Statement,
    TransactionTrait,
};
use uuid::Uuid;

use super::{jobs, notify};

pub use super::send::{Pending, dispatch_pending};

/// `agentic_runs.metadata.trigger` on every lever run (C4's `triggered_by`).
pub const TRIGGER: &str = "experiment";

pub struct RunCtx<'a> {
    pub run: &'a experiment_runs::Model,
    pub c: &'a ExperimentConfig,
    /// Now in `design.timezone`, floored to the hour (see `RunFacts::now`).
    pub local: oxy_experiment::Moment,
    pub now: DateTime<Utc>,
}

pub(super) fn exec(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// Record the transition's flips, then run its other actions.
pub async fn apply(db: &DatabaseConnection, ctx: &RunCtx<'_>, t: &Transition) -> Result<(), DbErr> {
    record_flips(db, ctx, t).await?;
    for action in &t.actions {
        match action {
            RunAction::DispatchFlip { .. } => {}
            RunAction::EnqueueEstimate => {
                jobs::enqueue_estimate(db, ctx.run).await?;
            }
            RunAction::NotifyOwner { reason } => {
                notify::notify_owner(db, ctx.run, &ctx.c.owner, reason).await;
            }
        }
    }
    Ok(())
}

const INSERT_FLIP: &str = "INSERT INTO experiment_flips \
     (id, run_id, unit, target_state, due_at, wave, status, created_at) \
     VALUES ($1, $2, $3, $4, $5, $6, 'pending', $7) \
     ON CONFLICT (run_id, unit, target_state, due_at) DO NOTHING";

/// A terminal `off` only when the unit has no uncancelled `off` on or after
/// its latest `on` (any, if it has no `on`) — `step`'s `needs_off`,
/// re-asserted under the run-row lock, because the row's date is the day it
/// was decided and the unique key cannot meet a second decision's.
const INSERT_TERMINAL_OFF: &str = "INSERT INTO experiment_flips \
     (id, run_id, unit, target_state, due_at, wave, status, created_at) \
     SELECT $1, $2, $3, $4, $5, $6, 'pending', $7 \
     WHERE NOT EXISTS (SELECT 1 FROM experiment_flips o \
       WHERE o.run_id = $2 AND o.unit = $3 AND o.target_state = 'off' AND o.status <> 'cancelled' \
         AND o.due_at >= COALESCE((SELECT MAX(i.due_at) FROM experiment_flips i \
              WHERE i.run_id = $2 AND i.unit = $3 AND i.target_state = 'on'), TIMESTAMP '0001-01-01 00:00:00')) \
     ON CONFLICT (run_id, unit, target_state, due_at) DO NOTHING";

/// Every flip of one transition in ONE transaction: the rows of a wave or a
/// switchback period are visible whole or not at all (Task 9a's shared task
/// relies on it). Into a terminal state, the run row's lock comes first, so
/// two replicas' terminal `off`s for one run are serialised.
async fn record_flips(
    db: &DatabaseConnection,
    ctx: &RunCtx<'_>,
    t: &Transition,
) -> Result<(), DbErr> {
    let flips: Vec<_> = t
        .actions
        .iter()
        .filter_map(|a| match a {
            RunAction::DispatchFlip {
                unit,
                state,
                wave,
                due_at,
            } => Some((unit, *state, *wave, *due_at)),
            _ => None,
        })
        .collect();
    if flips.is_empty() {
        return Ok(());
    }
    let terminal = t.next.is_terminal();
    let txn = db.begin().await?;
    if terminal {
        let lock = "SELECT id FROM experiment_runs WHERE id = $1 FOR UPDATE";
        txn.execute_raw(exec(lock, vec![ctx.run.id.into()])).await?;
    }
    for (unit, state, wave, due_at) in flips {
        let sql = if terminal && state == LeverState::Off {
            INSERT_TERMINAL_OFF
        } else {
            INSERT_FLIP
        };
        let values = vec![
            Uuid::new_v4().into(),
            ctx.run.id.into(),
            unit.as_str().into(),
            state.as_str().into(),
            due_at.at().into(),
            i32::try_from(wave).unwrap_or(i32::MAX).into(),
            ctx.now.fixed_offset().into(),
        ];
        txn.execute_raw(exec(sql, values)).await?;
    }
    txn.commit().await
}

/// On the way into `aborted`: an `on` that never left must never leave.
pub async fn cancel_undispatched_on(db: &DatabaseConnection, run_id: Uuid) -> Result<(), DbErr> {
    db.execute_raw(exec(
        "UPDATE experiment_flips SET status = 'cancelled', \
           reason = $2 \
         WHERE run_id = $1 AND target_state = 'on' AND task_id IS NULL AND status = 'pending'",
        vec![run_id.into(), super::send::RUN_ENDED.into()],
    ))
    .await?;
    Ok(())
}

#[derive(FromQueryResult)]
struct Ended {
    agentic_run_id: Uuid,
    unit: String,
    target_state: String,
    due_at: chrono::NaiveDateTime,
    task_status: Option<String>,
    error_message: Option<String>,
}

/// A sent batch whose lever run ended badly: every row it covered is a failed
/// flip, and the owner hears once per run, naming its units.
pub async fn sync_lever_outcomes(db: &DatabaseConnection, ctx: &RunCtx<'_>) -> Result<(), DbErr> {
    let ended = Ended::find_by_statement(exec(
        "SELECT f.agentic_run_id, f.unit, f.target_state, f.due_at, r.task_status, r.error_message \
         FROM experiment_flips f JOIN agentic_runs r ON r.id = f.agentic_run_id::text \
         WHERE f.run_id = $1 AND f.status = 'dispatched' \
           AND r.task_status IN ('failed', 'cancelled', 'timed_out') ORDER BY f.unit",
        vec![ctx.run.id.into()],
    ))
    .all(db)
    .await?;
    let mut by_run: BTreeMap<Uuid, Vec<Ended>> = BTreeMap::new();
    for e in ended {
        by_run.entry(e.agentic_run_id).or_default().push(e);
    }
    for (lever_run, rows) in by_run {
        let first = &rows[0];
        let reason = format!(
            "the lever automation run ended {}: {}",
            first.task_status.as_deref().unwrap_or("unknown"),
            first.error_message.as_deref().unwrap_or("no error message")
        );
        let changed = db
            .execute_raw(exec(
                "UPDATE experiment_flips SET status = 'failed', reason = $1 \
                 WHERE run_id = $2 AND agentic_run_id = $3 AND status = 'dispatched'",
                vec![reason.clone().into(), ctx.run.id.into(), lever_run.into()],
            ))
            .await?
            .rows_affected();
        if changed > 0 {
            let units: Vec<String> = rows.iter().map(|e| e.unit.clone()).collect();
            let notice = format!(
                "the {} flip due {} at {} failed: {reason}",
                first.target_state,
                oxy_experiment::Moment(first.due_at),
                super::send::named(&units)
            );
            notify::notify_owner(db, ctx.run, &ctx.c.owner, &notice).await;
        }
    }
    Ok(())
}
