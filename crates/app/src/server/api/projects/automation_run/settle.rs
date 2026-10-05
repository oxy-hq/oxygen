//! Closing a procedure run's row: **the first terminal state wins.** And
//! opening it for execution: **the first attempt to begin wins.**
//!
//! While the run was a spawn in the handler there was one writer per run and a
//! plain `UPDATE … WHERE id` was enough. On the queue there can be several: the
//! driver that finishes the run, a second attempt after a dead claim was
//! requeued, the cancel endpoint on any replica, and the poll endpoint closing a
//! run its driver gave up on. Every terminal write therefore goes through
//! [`close_running`], which only moves a row that is still `running`; and every
//! attempt goes through [`begin_execution`] before it runs a step, which only
//! stamps a row no attempt has begun.

use std::collections::HashMap;

use agentic_pipeline::automation_run::AutomationRunError;
use chrono::Utc;
use entity::customer_app_procedure_runs as proc_run;
use entity::customer_app_procedure_runs::ActiveModel as ProcRunActiveModel;
use sea_orm::{
    ActiveValue, ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter,
};
use serde_json::Value as JsonValue;
use uuid::Uuid;

/// The code a run closed by [`reconcile_abandoned`] carries. Not new on the
/// wire: it is what the two-hour sweep already writes for a run whose driver
/// is gone.
pub(super) const ORPHANED_CODE: &str = "automation_run_orphaned";

/// How long after it was accepted a row still `running` is failed by the
/// periodic sweep (`sweep_terminal_runs`), whoever is or is not driving it.
/// The longest any attempt can be left executing, so also how long a later
/// attempt keeps stepping aside for one (`executor::admission`).
pub(super) const ORPHAN_SWEEP_AFTER_SECS: i64 = 2 * 60 * 60;

/// The code a run carries when a later attempt found an earlier one had
/// already begun executing it, and declined to run it again.
pub const INTERRUPTED_CODE: &str = "automation_run_interrupted";

/// The message beside [`INTERRUPTED_CODE`]. The bundle's poll already handles
/// `failed`; this is the sentence its user reads.
pub const INTERRUPTED_MESSAGE: &str = "the automation was interrupted while running and was not \
                                       retried, to avoid repeating steps that already ran; start \
                                       it again";

/// A terminal state, as the columns it writes. No `id`: the row is named by
/// the `WHERE`, never by the `SET`.
pub(super) fn done(outputs: &HashMap<String, JsonValue>) -> ProcRunActiveModel {
    ProcRunActiveModel {
        status: ActiveValue::Set("done".into()),
        result_summary: ActiveValue::Set(Some(done_summary(outputs))),
        result_outputs: ActiveValue::Set(Some(
            serde_json::to_value(outputs).unwrap_or(JsonValue::Null),
        )),
        completed_at: ActiveValue::Set(Some(Utc::now().into())),
        ..Default::default()
    }
}

pub(super) fn done_summary(outputs: &HashMap<String, JsonValue>) -> String {
    if outputs.is_empty() {
        "Automation completed.".to_string()
    } else {
        format!("Automation completed — {} task outputs.", outputs.len())
    }
}

pub(super) fn failed(e: &AutomationRunError) -> ProcRunActiveModel {
    let (code, message) = super::automation_error_to_code(e);
    failed_with(code, message)
}

pub(super) fn failed_with(code: &str, message: String) -> ProcRunActiveModel {
    ProcRunActiveModel {
        status: ActiveValue::Set("failed".into()),
        error_message: ActiveValue::Set(Some(message)),
        error_code: ActiveValue::Set(Some(code.to_string())),
        completed_at: ActiveValue::Set(Some(Utc::now().into())),
        ..Default::default()
    }
}

pub(super) fn cancelled() -> ProcRunActiveModel {
    ProcRunActiveModel {
        status: ActiveValue::Set("cancelled".into()),
        error_message: ActiveValue::Set(Some("cancelled by user".into())),
        error_code: ActiveValue::Set(Some("automation_run_cancelled".into())),
        completed_at: ActiveValue::Set(Some(Utc::now().into())),
        ..Default::default()
    }
}

/// A run an earlier attempt began and did not finish, which this attempt
/// declines to run again.
pub(super) fn interrupted() -> ProcRunActiveModel {
    failed_with(INTERRUPTED_CODE, INTERRUPTED_MESSAGE.to_string())
}

/// Take the run for this attempt, or learn that it is not this attempt's to
/// run. One atomic write: stamp `execution_started_at` on a row that is still
/// `running`, that nobody has asked to cancel, and that no attempt has begun.
///
/// `Ok(true)`: this attempt owns the run and may start its first step — and it
/// is the only attempt that ever will, because the stamp it just wrote is what
/// every later attempt reads. `Ok(false)`: not ours. The row is closed, a
/// cancel is pending, or an earlier attempt already began it; the caller
/// re-reads the row to learn which (`executor::on_claim`).
///
/// The stamp goes on this table and not on the queue row because the queue's
/// `claim_count` is refunded by a graceful release and charged at claim time,
/// before anything ran — neither is "a step may have run". And it precedes the
/// runner rather than following it: a stamp written after the first step would
/// leave the crash window exactly where the double-run is.
///
/// The same write is the attempt's first heartbeat, so a begun run is never
/// without one: a later attempt reads this attempt as alive from the instant
/// it took the run (`executor::heartbeat` keeps it fresh from there).
pub(super) async fn begin_execution<C: ConnectionTrait>(
    db: &C,
    run_id: Uuid,
) -> Result<bool, DbErr> {
    let now: chrono::DateTime<chrono::FixedOffset> = Utc::now().into();
    let stamp = ProcRunActiveModel {
        execution_started_at: ActiveValue::Set(Some(now)),
        execution_heartbeat_at: ActiveValue::Set(Some(now)),
        ..Default::default()
    };
    let res = proc_run::Entity::update_many()
        .set(stamp)
        .filter(proc_run::Column::Id.eq(run_id))
        .filter(proc_run::Column::Status.eq("running"))
        .filter(proc_run::Column::CancelRequestedAt.is_null())
        .filter(proc_run::Column::ExecutionStartedAt.is_null())
        .exec(db)
        .await?;
    Ok(res.rows_affected > 0)
}

/// Which rows a terminal write may move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Guard {
    /// Any row still `running`.
    Running,
    /// Still `running` **and** nobody has asked to cancel it. A result must
    /// not land on a run its user cancelled — the cancel wins, as it did when
    /// the spawned task checked the stamp before recording its result.
    RunningAndNotCancelled,
}

/// Write `terminal` onto the run if `guard` still holds. `Ok(true)` when this
/// call closed the run, `Ok(false)` when it was not ours to close.
pub(super) async fn close_running<C: ConnectionTrait>(
    db: &C,
    run_id: Uuid,
    terminal: ProcRunActiveModel,
    guard: Guard,
) -> Result<bool, DbErr> {
    let mut update = proc_run::Entity::update_many()
        .set(terminal)
        .filter(proc_run::Column::Id.eq(run_id))
        .filter(proc_run::Column::Status.eq("running"));
    if guard == Guard::RunningAndNotCancelled {
        update = update.filter(proc_run::Column::CancelRequestedAt.is_null());
    }
    Ok(update.exec(db).await?.rows_affected > 0)
}

/// Close a `running` row whose driver has given up on it.
///
/// The recovery budget running out, the queue dead-lettering the task, and a
/// pod that predates this task kind refusing it all leave `agentic_runs`
/// terminal and never touch this table; without this the bundle would poll
/// `running` until the two-hour sweep.
///
/// A terminal `agentic_runs` status is not proof on its own, though, so the
/// run is closed only once nothing holds its task either ([`task_is_held`]).
/// What prompted the check is fixed at its source: `cleanup_stale_runs`, which
/// every `oxy serve` boot runs, used to fail a zero-event root whose entry a
/// driver had *claimed* — under that driver — and now spares it. The check
/// stays as the second lock: a pod still on the older build writes that
/// `failed` for the length of a rollout, and a close here cannot be taken
/// back — the driver's result is discarded and the user runs the steps again.
///
/// Returns the row to answer the poll with: re-read when this call (or anyone
/// else) closed it, the caller's own otherwise. A row with no `agentic_runs`
/// twin is a run started before the queue existed and is left alone.
pub(super) async fn reconcile_abandoned(
    db: &DatabaseConnection,
    row: proc_run::Model,
) -> proc_run::Model {
    if row.status != "running" {
        return row;
    }
    let Some(terminal) = abandoned_close(db, row.id).await else {
        return row;
    };
    match close_running(db, row.id, terminal, Guard::Running).await {
        Ok(_) => proc_run::Entity::find_by_id(row.id)
            .one(db)
            .await
            .ok()
            .flatten()
            .unwrap_or(row),
        Err(e) => {
            tracing::warn!(run_id = %row.id, error = %e, "procedure poll: closing an abandoned run failed");
            row
        }
    }
}

/// The terminal state to close an abandoned run with, or `None` when it is
/// not abandoned — or when that cannot be told. Never closes on a guess: a
/// failed lookup leaves the row for the next poll.
async fn abandoned_close(db: &DatabaseConnection, run_id: Uuid) -> Option<ProcRunActiveModel> {
    let run = match agentic_runtime::crud::get_run(db, &run_id.to_string()).await {
        Ok(run) => run?,
        Err(e) => {
            tracing::warn!(%run_id, error = %e, "procedure poll: driver-state lookup failed");
            return None;
        }
    };
    let terminal = match run.task_status.as_deref() {
        // The driver's own reason is Oxy's wording about Oxy's queue ("exceeded
        // 4 recovery attempts"), not something a bundle can act on. It goes to
        // the log; the bundle gets the sweep's code and a plain sentence.
        Some("failed") | Some("timed_out") => failed_with(
            ORPHANED_CODE,
            "automation was interrupted and could not be resumed".to_string(),
        ),
        Some("cancelled") => cancelled(),
        _ => return None,
    };
    match task_is_held(db, run_id).await {
        Ok(false) => {}
        Ok(true) => {
            tracing::info!(
                %run_id,
                task_status = run.task_status.as_deref().unwrap_or(""),
                "procedure poll: run reads terminal but its task is still held; leaving it open"
            );
            return None;
        }
        Err(e) => {
            tracing::warn!(%run_id, error = %e, "procedure poll: queue lookup failed");
            return None;
        }
    }
    tracing::warn!(
        %run_id,
        driver_error = run.error_message.as_deref().unwrap_or(""),
        "procedure run was abandoned by its driver; closing it"
    );
    Some(terminal)
}

/// Whether an attempt may still drive the run: its queue entry is held by a
/// claim, or waiting for one (a reaped claim goes back to `queued`, and the
/// next claimant settles the row itself). An absent entry, or one in any other
/// state, holds nothing. One primary-key read, and only for a `running` row
/// whose run reads terminal.
async fn task_is_held(db: &DatabaseConnection, run_id: Uuid) -> Result<bool, DbErr> {
    let entry = agentic_runtime::crud::get_queue_entry(db, &run_id.to_string()).await?;
    Ok(entry.is_some_and(|q| matches!(q.queue_status.as_str(), "queued" | "claimed")))
}

#[cfg(test)]
mod tests;
