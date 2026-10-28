//! Answers and cancels for an analytics run that no process in this request's
//! reach is driving: through the queue and the run row, not through channels
//! in some process's memory.
//!
//! A root driven from the queue stops at a question to a person: recovery
//! ends its claim and its driver lease and leaves it `awaiting_input` with
//! its checkpoint (`recovery.rs`, `park`). Nothing is waiting on an answer
//! channel for it anywhere. [`queue_answer`] is how the answer reaches it from
//! any replica:
//!
//! 1. The run is taken from `awaiting_input` back to `running` — one guarded
//!    statement, so two answers cannot both continue it, and a run a live
//!    driver still holds is never continued under that driver.
//! 2. The `input_resolved` event is written, as an in-process answer writes
//!    it, so the stream and a reload see the question answered.
//! 3. The continuation is queued as the root's own entry: a `TaskSpec::Resume`
//!    from the checkpoint the run is at, carrying the answer, `Global`.
//!
//! All three in one transaction: a run left `running` with nothing queued
//! would be a run no driver ever selects. Whichever driver's latency worker
//! picks it up next runs the resume from that checkpoint
//! (`recovery/root_entry.rs`: the entry *is* the continuation) as the run's
//! recorded requester — never from the top, and without spending the run's
//! recovery budget: answering a question is not a recovery.
//!
//! [`close_if_undriven`] is the cancel half, shared by every route that
//! cancels a run: the durable flag reaches a driver in another process, and
//! the row is closed here only when nothing can be driving it.

use agentic_core::delegation::TaskSpec;
use agentic_runtime::crud;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement, TransactionTrait,
};

/// What [`queue_answer`] did with an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueuedAnswer {
    /// The continuation is on the queue.
    Queued,
    /// The run is not waiting on a person: answered already, running, ended.
    /// Carries the status it is in.
    NotWaiting(Option<String>),
    /// The run is waiting, but a live driver holds its lease — a process that
    /// drives it in memory and will take the answer only through its own
    /// channel. Continuing it here would be a second drive.
    DrivenElsewhere,
    /// The run is waiting but its checkpoint is gone or unreadable: there is
    /// nothing to continue from.
    NoCheckpoint,
}

/// Deliver `answer` to the root run `run_id`, which is parked on a person with
/// no driver, by queuing its continuation. See the module docs.
pub async fn queue_answer(
    db: &DatabaseConnection,
    run_id: &str,
    answer: &str,
) -> Result<QueuedAnswer, DbErr> {
    let txn = db.begin().await?;
    let Some(resume_data) = crud::get_suspension(&txn, run_id).await? else {
        return Ok(QueuedAnswer::NoCheckpoint);
    };
    // Waiting, a root, and nobody's: the one state an answer may continue.
    let taken = txn
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE agentic_runs \
                SET task_status = 'running', updated_at = now() \
              WHERE id = $1 \
                AND parent_run_id IS NULL \
                AND task_status = 'awaiting_input' \
                AND (driver_id IS NULL \
                     OR driver_heartbeat_at IS NULL \
                     OR driver_heartbeat_at < now() - make_interval(secs => $2))",
            [run_id.into(), (crud::DRIVER_LEASE_TTL_SECS as i32).into()],
        ))
        .await?
        .rows_affected();
    if taken == 0 {
        drop(txn);
        return why_not(db, run_id).await;
    }
    let run = crud::get_run(db, run_id).await?;
    let attempt = run.as_ref().map(|r| r.attempt).unwrap_or(0);
    let seq = crud::get_max_seq(&txn, run_id).await? + 1;
    crud::insert_event(
        &txn,
        run_id,
        seq,
        "input_resolved",
        &serde_json::json!({ "answer": answer, "trace_id": resume_data.trace_id }),
        attempt,
    )
    .await?;
    crud::enqueue_task(
        &txn,
        run_id,
        run_id,
        None,
        &TaskSpec::Resume {
            run_id: run_id.to_string(),
            resume_data,
            answer: answer.to_string(),
        },
        None,
        crud::TaskScope::Global,
    )
    .await?;
    // The enqueue's upsert keeps an existing row's `scope_owned`, and a root
    // answered in process before has a scoped row (its coordinator's
    // transport assigned the resume `Scoped`). A scoped row is invisible to
    // every latency worker, so say `Global` again, here, in the same
    // transaction.
    txn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_task_queue SET scope_owned = false WHERE task_id = $1",
        [run_id.into()],
    ))
    .await?;
    txn.commit().await?;
    Ok(QueuedAnswer::Queued)
}

/// Why the guarded statement took nothing, read after the fact.
async fn why_not(db: &DatabaseConnection, run_id: &str) -> Result<QueuedAnswer, DbErr> {
    let Some(run) = crud::get_run(db, run_id).await? else {
        return Ok(QueuedAnswer::NotWaiting(None));
    };
    if run.task_status.as_deref() == Some("awaiting_input") && run.parent_run_id.is_none() {
        return Ok(QueuedAnswer::DrivenElsewhere);
    }
    Ok(QueuedAnswer::NotWaiting(run.task_status))
}

/// Fail `run_id` with `message` if nothing can be driving it and it has not
/// ended; otherwise write nothing. Returns whether the row was closed here.
///
/// For a cancel, after the durable flag (`crud::request_cancel`) is written and
/// no driver in this process took the signal. A run whose queue entry is
/// `queued` or `claimed` is left to whoever holds or takes that entry — its
/// driver turns the flag into a proper `cancelled`. Otherwise one guarded
/// statement closes it (`crud::fail_undriven_run`: not terminal, no live
/// lease), so a run that finishes or is claimed in between is still never
/// written over, and a finished run stays as it finished.
pub async fn close_if_undriven(
    db: &DatabaseConnection,
    run_id: &str,
    message: &str,
) -> Result<bool, DbErr> {
    match crud::get_queue_entry(db, run_id).await {
        Ok(Some(entry)) if matches!(entry.queue_status.as_str(), "queued" | "claimed") => {
            return Ok(false);
        }
        Ok(_) => {}
        // Unknown. The flag is already written and is what stops a live run;
        // a wrong `failed` on one is not recoverable, so do not write.
        Err(e) => {
            tracing::warn!(%run_id, error = %e, "cancel: queue lookup failed; leaving the run row alone");
            return Ok(false);
        }
    }
    crud::fail_undriven_run(db, run_id, message).await
}
