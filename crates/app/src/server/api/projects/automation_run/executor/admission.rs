//! May this attempt run the automation? A claimed attempt reads the run's row
//! ([`admit`]), takes it with the stamp immediately before the first step
//! ([`begin`]), and otherwise settles what it read — or, when the run is not
//! its to settle, hands the task back unrun ([`step_aside`]).
//!
//! A run another attempt already began is the case that needs care, because
//! the stamp reads the same whether that attempt is dead or still running the
//! steps. Its heartbeat ([`super::heartbeat`]) tells them apart: stale, and
//! the run is closed as interrupted; fresh, and this attempt steps aside for
//! as long as it stays fresh.

use agentic_core::delegation::TaskOutcome;
use chrono::{DateTime, Utc};
use entity::customer_app_procedure_runs as proc_run;
use sea_orm::{DatabaseConnection, EntityTrait};
use uuid::Uuid;

use super::{close_cancelled, close_interrupted, heartbeat, settle};

/// What a claimed attempt should do, given the run's row.
#[derive(Debug)]
pub(super) enum ClaimStep {
    /// Still `running`, never begun, nobody cancelled it: take it and execute.
    Run,
    /// Cancelled before this attempt started. Close it and stop.
    Cancel,
    /// Still `running`, begun by another attempt, and that attempt's heartbeat
    /// is fresh: it is executing the steps now. Not this attempt's to run and
    /// not its to close — step aside.
    Live,
    /// Still `running`, begun by another attempt, and that attempt's heartbeat
    /// has gone stale — its driver died or was replaced mid-run. Close it as
    /// interrupted; do **not** run it again.
    Interrupted,
    /// Already closed by an earlier attempt, the cancel endpoint or the sweep.
    /// Touch nothing; report the state it was closed in.
    Closed(Box<TaskOutcome>),
}

/// Read the row's answer to "may this attempt run?", as of `now`.
///
/// Split from the job so the mapping is assertable without a database —
/// which arm an attempt takes is the whole of this executor's idempotency.
/// A cancel outranks an interruption: a run its user stopped reads as stopped,
/// whatever happened to its driver.
pub(super) fn on_claim(row: Option<&proc_run::Model>, now: DateTime<Utc>) -> ClaimStep {
    let Some(row) = row else {
        return ClaimStep::Closed(Box::new(TaskOutcome::Failed(
            "procedure run row not found".to_string(),
        )));
    };
    match row.status.as_str() {
        "running" if row.cancel_requested_at.is_some() => ClaimStep::Cancel,
        "running" => match heartbeat::last_alive(row) {
            None => ClaimStep::Run,
            Some(last_alive) if heartbeat::is_stale(last_alive, now) => ClaimStep::Interrupted,
            Some(_) => ClaimStep::Live,
        },
        "cancelled" => ClaimStep::Closed(Box::new(TaskOutcome::Cancelled)),
        // `Done`, not `Failed`: the work is finished and the queue must not
        // hand this payload out again.
        _ => ClaimStep::Closed(Box::new(TaskOutcome::Done {
            answer: "run was already closed by an earlier attempt".to_string(),
            metadata: None,
        })),
    }
}

/// How long an attempt that steps aside withholds the run from the queue.
/// Longer than a pod's termination grace (30s by default), so an attempt still
/// finishing on a pod being replaced has closed the run, or been killed, by
/// the time the next claimant reads it — and no shorter than the heartbeat's
/// staleness threshold, so an attempt that died right after this one read it
/// is stale at the very next claim rather than a round later.
const STEP_ASIDE_DELAY_SECS: u64 = 60;
const _: () = assert!(STEP_ASIDE_DELAY_SECS as i64 >= heartbeat::STALE_AFTER_SECS);

/// How long a run may keep being handed back before the queue dead-letters
/// it, which retires the run and lets the poll close it as orphaned — under
/// whoever is still running it. So it is the orphan sweep's own two hours
/// (`settle::ORPHAN_SWEEP_AFTER_SECS`): the sweep closes any row still
/// `running` that long after it was accepted, and a streak of deferrals
/// cannot start before the row does, so stepping aside never cuts a live
/// attempt shorter than the sweep already would.
///
/// One ceiling for every reason to step aside, on purpose. The queue measures
/// it from the first deferral of the streak against whichever call's value
/// arrives (`crud::defer_task`), so a shorter ceiling on one reason would
/// dead-letter a streak a live attempt's heartbeat had been extending.
const STEP_ASIDE_MAX_WAIT_SECS: u64 = settle::ORPHAN_SWEEP_AFTER_SECS as u64;

/// Hand the task back unrun and record nothing: the run is not this attempt's
/// to settle. `Deferred` is the one outcome that writes no terminal
/// `agentic_runs` status, so neither the poll nor the queue reads a refused
/// attempt as the end of a run another attempt may be driving.
fn step_aside(run_id: Uuid, reason: String) -> TaskOutcome {
    tracing::info!(%run_id, %reason, "procedure run: stepping aside for a later claim");
    TaskOutcome::Deferred {
        delay_secs: STEP_ASIDE_DELAY_SECS,
        max_wait_secs: STEP_ASIDE_MAX_WAIT_SECS,
        reason,
    }
}

async fn read_row(
    db: &DatabaseConnection,
    run_id: Uuid,
) -> Result<Option<proc_run::Model>, TaskOutcome> {
    proc_run::Entity::find_by_id(run_id)
        .one(db)
        .await
        // Not settled on a failed read: the row may be fine and the lookup a
        // blip, and another attempt may be driving it.
        .map_err(|e| step_aside(run_id, format!("procedure run lookup failed: {e}")))
}

/// May this attempt run the automation at all? Read the row and classify it;
/// `Err` is the attempt's final outcome, settled here. Does not take the run —
/// that is [`begin`], once there is something to run.
pub(super) async fn admit(db: &DatabaseConnection, run_id: Uuid) -> Result<(), TaskOutcome> {
    let row = read_row(db, run_id).await?;
    match on_claim(row.as_ref(), Utc::now()) {
        ClaimStep::Run => Ok(()),
        step => Err(resolve(db, run_id, step).await),
    }
}

/// Take the run for this attempt: the stamp, immediately before the first
/// step. Refused when the row moved since [`admit`] — a cancel landed, or
/// another attempt began — and [`refused`] says what to do. `Ok` is the one
/// way into the runner; every `Err` is the attempt's final outcome.
///
/// After `prepare` rather than before it, so a driver that dies while
/// building the context leaves a run no step of which has run, which the next
/// attempt may run.
pub(super) async fn begin(db: &DatabaseConnection, run_id: Uuid) -> Result<(), TaskOutcome> {
    match settle::begin_execution(db, run_id).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(refused(db, run_id).await),
        // The stamp may or may not have landed; the next claimant reads which.
        Err(e) => Err(step_aside(
            run_id,
            format!("procedure run could not be taken: {e}"),
        )),
    }
}

/// Settle an attempt whose stamp was refused: the row moved since [`admit`],
/// and the re-read says how, by the same rule as admission. The usual reading
/// is [`ClaimStep::Live`] — the attempt that took the run wrote its heartbeat
/// with the stamp, moments ago — and closing the run under it would discard
/// its result and send the user to run its side effects a second time.
async fn refused(db: &DatabaseConnection, run_id: Uuid) -> TaskOutcome {
    match read_row(db, run_id).await {
        Ok(row) => resolve(db, run_id, on_claim(row.as_ref(), Utc::now())).await,
        Err(outcome) => outcome,
    }
}

/// Settle an attempt that is not going to run the automation.
pub(super) async fn resolve(db: &DatabaseConnection, run_id: Uuid, step: ClaimStep) -> TaskOutcome {
    match step {
        // Only after a refused stamp ([`admit`] runs a runnable row): the row
        // reads as runnable although the stamp just said otherwise, which no
        // writer of this table produces. Not this attempt's to settle either.
        ClaimStep::Run => step_aside(
            run_id,
            "the run reads as unbegun after its stamp was refused".to_string(),
        ),
        ClaimStep::Cancel => close_cancelled(db, run_id).await,
        // Writes nothing. The next claim reads the row again: closed if that
        // attempt finished, interrupted once its heartbeat has stopped.
        ClaimStep::Live => step_aside(
            run_id,
            "another attempt is executing the run (its heartbeat is fresh)".to_string(),
        ),
        ClaimStep::Interrupted => {
            tracing::warn!(
                %run_id,
                "procedure run was interrupted by an earlier attempt; closing it, not re-running it"
            );
            close_interrupted(db, run_id).await
        }
        ClaimStep::Closed(outcome) => {
            tracing::info!(%run_id, "procedure run is already closed; not re-running it");
            *outcome
        }
    }
}
