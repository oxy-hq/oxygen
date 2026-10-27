//! Sending recorded flips. A batch (`batch.rs`) goes out as ONE lever task in
//! ONE transaction: stamp the batch's rows, seed the task, commit. Stamping
//! first is what makes it exactly-once across ticks, replicas and crashes —
//! the argument is Task 9a's, step by step, and `seed_batch` is its step 2.

use std::collections::HashSet;

use agentic_pipeline::TaskScope;
use agentic_pipeline::automation_run::{AutomationRunError, StartAutomationRequest};
use agentic_pipeline::automation_seed::seed_automation_run;
use entity::experiment_flips;
use oxy_experiment::run::LeverState;
use oxy_experiment::timeline::moment_utc;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, FromQueryResult, TransactionTrait};
use serde_json::Value;
use uuid::Uuid;

use super::batch::{self, Batch, period_span};
use super::dispatch::{RunCtx, TRIGGER, exec};
use super::{lever, notify, store};

const TERMINAL: [&str; 4] = ["done", "failed", "cancelled", "timed_out"];

#[derive(Debug, Default)]
pub struct Pending {
    /// Rows sent (a `per: wave` batch counts each of its rows).
    pub sent: usize,
    /// Batches waiting on a predecessor or on the pinned membership.
    pub deferred: usize,
}

/// Send every recorded, unsent batch — this tick's and any crash orphan.
pub async fn dispatch_pending(db: &DatabaseConnection, ctx: &RunCtx<'_>) -> Result<Pending, DbErr> {
    let mut out = Pending::default();
    if cancel_ended_periods(db, ctx, &store::flips_of(db, ctx.run.id).await?).await? > 0 {
        tracing::info!(target: "experiment", run_id = %ctx.run.id, "ended switchback periods cancelled, not replayed");
    }
    let rows = store::flips_of(db, ctx.run.id).await?;
    let unsent: Vec<_> = rows
        .iter()
        .filter(|f| f.status == "pending" && f.task_id.is_none())
        .collect();
    let membership = store::group_membership(ctx.run);
    for b in batch::batches(ctx.c, &unsent) {
        let vars = if batch_settled(db, &rows, &b).await? {
            batch::variables(ctx.c, &ctx.c.experiment, &b, membership.as_ref())
        } else {
            Ok(None)
        };
        match vars {
            Ok(Some(vars)) => out.sent += send_batch(db, ctx, &b, vars).await?,
            Ok(None) => out.deferred += 1,
            Err(reason) => fail_batch(db, ctx, &b, &reason).await?,
        }
    }
    Ok(out)
}

pub const PERIOD_ENDED: &str = "period ended before dispatch";
/// The reason on an `on` flip the run ended (aborted or reported) before it
/// was dispatched. Not a period: a switchback's count never includes it.
pub const RUN_ENDED: &str = "run ended before dispatch";

/// C2b: a switchback never replays a period that has ended. Every unsent row
/// whose period is over — recorded late after an outage, or held behind its
/// predecessor past the period's end — is cancelled, never sent.
async fn cancel_ended_periods(
    db: &DatabaseConnection,
    ctx: &RunCtx<'_>,
    rows: &[experiment_flips::Model],
) -> Result<u64, DbErr> {
    let Some(sb) = &ctx.c.design.switchback else {
        return Ok(0);
    };
    let ended: Vec<Uuid> = rows
        .iter()
        .filter(|f| f.status == "pending" && f.task_id.is_none())
        .filter(|f| period_span(sb, ctx.c.design.grain, f).is_some_and(|p| p.until <= ctx.local))
        .map(|f| f.id)
        .collect();
    if ended.is_empty() {
        return Ok(0);
    }
    let result = db
        .execute_raw(exec(
            "UPDATE experiment_flips SET status = 'cancelled', reason = $1 \
             WHERE id = ANY($2) AND task_id IS NULL AND status = 'pending'",
            vec![PERIOD_ENDED.into(), ended.into()],
        ))
        .await?;
    Ok(result.rows_affected())
}

async fn send_batch(
    db: &DatabaseConnection,
    ctx: &RunCtx<'_>,
    b: &Batch,
    vars: Value,
) -> Result<usize, DbErr> {
    let Some(via) = lever::via_for(ctx.c, b.state) else {
        return fail_batch(db, ctx, b, "the design has no lever")
            .await
            .map(|()| 0);
    };
    match lever::resolve_for_runner(db, ctx.run.workspace_id, via).await? {
        Ok(lever) => seed_batch(db, ctx, b, &lever, vars).await,
        Err(reason) => fail_batch(db, ctx, b, &reason).await.map(|()| 0),
    }
}

const STAMP: &str = "UPDATE experiment_flips SET task_id = $1, agentic_run_id = $1, \
       lever_revision_id = $2, status = 'dispatched' \
     WHERE id = ANY($3) AND task_id IS NULL AND status = 'pending' \
       AND (target_state <> 'on' OR NOT EXISTS (SELECT 1 FROM experiment_runs r \
         WHERE r.id = experiment_flips.run_id AND (r.state IN ('aborted', 'reported') \
           OR r.abort_reason IS NOT NULL))) RETURNING unit";

#[derive(FromQueryResult)]
struct Stamped {
    unit: String,
}

/// Stamp, then seed, then commit. The stamp's row locks serialise every
/// sender of this batch; a second one returns no units and rolls back. A
/// return that is not the whole batch is rolled back too and left to the
/// next tick, so no unit is ever told twice or told without its row stamped.
async fn seed_batch(
    db: &DatabaseConnection,
    ctx: &RunCtx<'_>,
    b: &Batch,
    lever: &lever::ResolvedLever,
    vars: Value,
) -> Result<usize, DbErr> {
    let task = Uuid::new_v4();
    let txn = db.begin().await?;
    let stamp = exec(
        STAMP,
        vec![task.into(), lever.revision_id.into(), b.ids.clone().into()],
    );
    let mut units: Vec<String> = Stamped::find_by_statement(stamp)
        .all(&txn)
        .await?
        .into_iter()
        .map(|s| s.unit)
        .collect();
    units.sort();
    if units != b.units {
        txn.rollback().await?;
        if b.state == LeverState::On {
            cancel_if_run_ended(db, b).await?;
        }
        return Ok(0);
    }
    let request = lever_request(ctx, b, &lever.file_path, vars);
    match seed_automation_run(
        &txn,
        &task.to_string(),
        request,
        TaskScope::Global,
        ctx.run.workspace_id,
    )
    .await
    {
        Ok(()) => {
            txn.commit().await?;
            Ok(units.len())
        }
        Err(AutomationRunError::Db(e)) => {
            txn.rollback().await?;
            Err(e)
        }
        Err(refused) => {
            txn.rollback().await?;
            fail_batch(db, ctx, b, &refused.to_string())
                .await
                .map(|()| 0)
        }
    }
}

/// A late `on` batch (recorded by a tick that stalled past its lease, after
/// another replica aborted the run) lost the stamp's fence: it must never go
/// out and must not stay `pending` forever.
async fn cancel_if_run_ended(db: &DatabaseConnection, b: &Batch) -> Result<(), DbErr> {
    db.execute_raw(exec(
        "UPDATE experiment_flips SET status = 'cancelled', reason = $2 \
         WHERE id = ANY($1) AND task_id IS NULL AND status = 'pending' \
           AND EXISTS (SELECT 1 FROM experiment_runs r \
             WHERE r.id = experiment_flips.run_id \
               AND (r.state IN ('aborted', 'reported') OR r.abort_reason IS NOT NULL))",
        vec![b.ids.clone().into(), RUN_ENDED.into()],
    ))
    .await?;
    Ok(())
}

fn lever_request(
    ctx: &RunCtx<'_>,
    b: &Batch,
    workflow_ref: &str,
    vars: Value,
) -> StartAutomationRequest {
    StartAutomationRequest {
        workflow_ref: workflow_ref.to_string(),
        variables: Some(vars),
        retry_from_run_id: None,
        cache_enabled: false,
        invalidate_steps: None,
        invalidate_iterations: None,
        thread_id: None,
        schedule_id: None,
        trigger: Some(TRIGGER.to_string()),
        logical_date: Some(moment_utc(b.due_at, ctx.c.design.timezone)),
        retry_of: None,
    }
}

/// `` `bondi`, `newtown` `` — the units a notice names.
pub(super) fn named(units: &[String]) -> String {
    units
        .iter()
        .map(|u| format!("`{u}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

async fn fail_batch(
    db: &DatabaseConnection,
    ctx: &RunCtx<'_>,
    b: &Batch,
    reason: &str,
) -> Result<(), DbErr> {
    let changed = db
        .execute_raw(exec(
            "UPDATE experiment_flips SET status = 'failed', reason = $1 \
             WHERE id = ANY($2) AND task_id IS NULL AND status = 'pending'",
            vec![reason.into(), b.ids.clone().into()],
        ))
        .await?
        .rows_affected();
    tracing::warn!(
        target: "experiment", run_id = %ctx.run.id, units = %named(&b.units),
        state = b.state.as_str(), %reason, "lever flip failed"
    );
    if changed > 0 {
        let notice = format!(
            "the {} flip due {} at {} failed: {reason}",
            b.state.as_str(),
            b.due_at,
            named(&b.units)
        );
        notify::notify_owner(db, ctx.run, &ctx.c.owner, &notice).await;
    }
    Ok(())
}

/// The unit's latest flip of the OTHER state due on or before `flip`.
fn predecessor<'a>(
    rows: &'a [experiment_flips::Model],
    flip: &experiment_flips::Model,
) -> Option<&'a experiment_flips::Model> {
    rows.iter()
        .filter(|r| r.unit == flip.unit && r.target_state != flip.target_state)
        .filter(|r| r.due_at <= flip.due_at)
        .max_by_key(|r| (r.due_at, r.created_at))
}

#[derive(FromQueryResult)]
struct Ended {
    task_status: Option<String>,
}

/// Task 9's rule for every row of the batch: no predecessor still waiting to
/// be sent, and every predecessor's lever run terminal — one query for the
/// distinct runs, however many units the batch holds.
async fn batch_settled(
    db: &impl ConnectionTrait,
    rows: &[experiment_flips::Model],
    b: &Batch,
) -> Result<bool, DbErr> {
    let ids: HashSet<Uuid> = b.ids.iter().copied().collect();
    let prev: Vec<_> = rows
        .iter()
        .filter(|r| ids.contains(&r.id))
        .filter_map(|r| predecessor(rows, r))
        .collect();
    if prev.iter().any(|p| p.status == "pending") {
        return Ok(false);
    }
    let runs: HashSet<String> = prev
        .iter()
        .filter_map(|p| p.agentic_run_id)
        .map(|id| id.to_string())
        .collect();
    if runs.is_empty() {
        return Ok(true);
    }
    let statuses = Ended::find_by_statement(exec(
        "SELECT task_status FROM agentic_runs WHERE id = ANY($1)",
        vec![runs.into_iter().collect::<Vec<_>>().into()],
    ))
    .all(db)
    .await?;
    // A run row that is gone cannot still be running; only one present and not terminal waits.
    Ok(statuses.iter().all(|s| {
        s.task_status
            .as_deref()
            .is_some_and(|t| TERMINAL.contains(&t))
    }))
}
