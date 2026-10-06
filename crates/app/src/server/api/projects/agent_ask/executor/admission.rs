//! May this attempt start the ask's pipeline? A claimed attempt reads the
//! run ([`admit`]), takes it with the stamp immediately before the pipeline
//! starts ([`begin`]), and otherwise settles what it read — or, when the run
//! is not its to settle, hands the task back unrun ([`step_aside`]).
//!
//! The rule it enforces is the one recovery already applies to an analytics
//! run: **resume from a suspension checkpoint, otherwise close as interrupted,
//! never re-run.** An attempt holding the ask's *start* spec is only ever the
//! first of those three when nothing has begun the run; everything else it can
//! find is somebody else's:
//!
//! * a run with a suspension is continued from it by recovery
//!   (`agentic_pipeline::recovery`, which holds this entry back so a fresh
//!   recovery never hands it out) — [`ClaimStep::Parked`];
//! * a run another attempt began and is still executing is that attempt's —
//!   [`ClaimStep::Live`];
//! * a run another attempt began and died in has no checkpoint to resume from
//!   and must not be started again — [`ClaimStep::Interrupted`].

use agentic_core::delegation::TaskOutcome;
use agentic_pipeline::run_execution::RunExecution;
use agentic_runtime::entity::run;
use chrono::{DateTime, Utc};
use sea_orm::DatabaseConnection;

use super::{Settled, heartbeat};

/// The message a run carries when a later attempt found an earlier one had
/// begun it and died, and declined to run it again. The bundle's stream ends
/// on it as an `error`; this is the sentence its user reads.
pub const INTERRUPTED_MESSAGE: &str = "the agent was interrupted while answering and was not \
                                       restarted, to avoid repeating work it had already done; \
                                       ask again";

/// What a claimed attempt should do, given what the run's rows say.
#[derive(Debug)]
pub(super) enum ClaimStep {
    /// Open, never begun, never suspended, nobody cancelled it: take it.
    Run,
    /// Cancelled before this attempt started. Close it and stop.
    Cancel,
    /// At a suspension: waiting on a person or on its children, or resumable
    /// from that checkpoint. Not this attempt's to run — that would start it
    /// again from the top — and not its to close.
    Parked,
    /// Begun by another attempt whose heartbeat is fresh: it is running the
    /// pipeline now. Not this attempt's to run and not its to close.
    Live,
    /// Begun by another attempt whose heartbeat has gone stale, with no
    /// checkpoint to resume from. Close it as interrupted; do **not** run it.
    Interrupted,
    /// Already ended. Touch nothing; report the state it ended in.
    Closed(Box<TaskOutcome>),
}

/// Read the rows' answer to "may this attempt run?", as of `now`.
///
/// Split from the database so the mapping is assertable without one — which
/// arm an attempt takes is the whole of this executor's idempotency. A cancel
/// outranks everything short of an ended run: an ask its user stopped reads as
/// stopped, whatever happened to its driver.
///
/// An ended run is reported as it ended, with its own answer or error. The
/// coordinator writes an outcome onto the run it belongs to, so anything else
/// here would rewrite a finished run.
pub(super) fn on_claim(
    run: Option<&run::Model>,
    execution: Option<RunExecution>,
    suspended: bool,
    now: DateTime<Utc>,
) -> ClaimStep {
    let Some(run) = run else {
        return closed(TaskOutcome::Failed("ask run row not found".to_string()));
    };
    match run.task_status.as_deref() {
        Some("done") => {
            return closed(TaskOutcome::Done {
                answer: run.answer.clone().unwrap_or_default(),
                metadata: None,
            });
        }
        Some("cancelled") => return closed(TaskOutcome::Cancelled),
        Some("failed") | Some("timed_out") => {
            let error = run.error_message.clone();
            return closed(TaskOutcome::Failed(
                error.unwrap_or_else(|| "the run had already failed".to_string()),
            ));
        }
        _ => {}
    }
    if run.cancel_requested_at.is_some() {
        return ClaimStep::Cancel;
    }
    if suspended {
        return ClaimStep::Parked;
    }
    // No extension row: nowhere to take the stamp, so nothing may start.
    let Some(execution) = execution else {
        return closed(TaskOutcome::Failed(
            "the run has no analytics extension row to take".to_string(),
        ));
    };
    match execution.last_alive() {
        None => ClaimStep::Run,
        Some(last_alive) if heartbeat::is_stale(last_alive, now) => ClaimStep::Interrupted,
        Some(_) => ClaimStep::Live,
    }
}

fn closed(outcome: TaskOutcome) -> ClaimStep {
    ClaimStep::Closed(Box::new(outcome))
}

/// How long an attempt that steps aside withholds the ask from the queue. No
/// shorter than the heartbeat's staleness threshold, so an attempt that died
/// right after this one read it is stale at the very next claim rather than a
/// round later.
const STEP_ASIDE_DELAY_SECS: u64 = 60;
const _: () = assert!(STEP_ASIDE_DELAY_SECS as i64 >= heartbeat::STALE_AFTER_SECS);

/// How long an ask may keep being handed back before the queue dead-letters
/// it, which retires the run. A day: nothing an ask does takes one, and it is
/// comfortably past the coordinator's own ceiling on a delegation (4 h), so a
/// run parked on its children is failed by the coordinator that owns it — with
/// a message naming them — long before this would retire it.
///
/// One ceiling for every reason to step aside, on purpose. The queue measures
/// it from the first deferral of the streak against whichever call's value
/// arrives (`crud::defer_task`), so a shorter ceiling on one reason would
/// dead-letter a streak another reason had been extending.
const STEP_ASIDE_MAX_WAIT_SECS: u64 = 24 * 60 * 60;
const _: () = assert!(
    STEP_ASIDE_MAX_WAIT_SECS > agentic_runtime::coordinator::DEFAULT_SUSPEND_TIMEOUT.as_secs()
);

/// Hand the task back unrun and record nothing: the run is not this attempt's
/// to settle, or not yet. `Deferred` is the one outcome that writes nothing
/// onto the run, so the stream never reads a refused attempt as the end of an
/// ask another attempt may be driving — or one this attempt may still run.
pub(super) fn step_aside(run_id: &str, reason: String) -> Settled {
    tracing::info!(%run_id, %reason, "agent ask: stepping aside for a later claim");
    Settled::only(TaskOutcome::Deferred {
        delay_secs: STEP_ASIDE_DELAY_SECS,
        max_wait_secs: STEP_ASIDE_MAX_WAIT_SECS,
        reason,
    })
}

/// The three reads [`on_claim`] needs. A failed read settles nothing: the
/// rows may be fine and the lookup a blip, and another attempt may be driving.
async fn read(
    db: &DatabaseConnection,
    run_id: &str,
) -> Result<(Option<run::Model>, ClaimStep), Settled> {
    let lookup_failed = |what: &str, e: sea_orm::DbErr| {
        step_aside(run_id, format!("agent ask {what} lookup failed: {e}"))
    };
    let run = agentic_runtime::crud::get_run(db, run_id)
        .await
        .map_err(|e| lookup_failed("run", e))?;
    let execution = agentic_pipeline::run_execution::get_run_execution(db, run_id)
        .await
        .map_err(|e| lookup_failed("execution", e))?;
    let suspended = agentic_runtime::crud::get_suspension(db, run_id)
        .await
        .map_err(|e| lookup_failed("suspension", e))?
        .is_some();
    let step = on_claim(run.as_ref(), execution, suspended, Utc::now());
    Ok((run, step))
}

/// May this attempt run the ask at all? `Ok` is the run to drive; `Err` is the
/// attempt's final report. Does not take the run — that is [`begin`], once
/// there is a pipeline to start.
pub(super) async fn admit(db: &DatabaseConnection, run_id: &str) -> Result<run::Model, Settled> {
    match read(db, run_id).await? {
        (Some(run), ClaimStep::Run) => Ok(run),
        (_, step) => Err(resolve(run_id, step)),
    }
}

/// Take the run for this attempt: the stamp, immediately before the pipeline
/// starts. Refused when the rows moved since [`admit`] — a cancel landed, or
/// another attempt began — and the re-read says what to report. `Ok` is the
/// one way into the pipeline.
///
/// After the context is built rather than before it, so a driver that dies
/// while building it leaves a run nothing has started, which the next attempt
/// may run.
pub(super) async fn begin(db: &DatabaseConnection, run_id: &str) -> Result<(), Settled> {
    match agentic_pipeline::run_execution::begin_execution(db, run_id).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(refused(db, run_id).await),
        // The stamp may or may not have landed; the next claimant reads which.
        Err(e) => Err(step_aside(
            run_id,
            format!("agent ask could not be taken: {e}"),
        )),
    }
}

/// Settle an attempt whose stamp was refused, by the same rule as admission.
/// The usual reading is [`ClaimStep::Live`] — the attempt that took the run
/// wrote its heartbeat with the stamp, moments ago.
async fn refused(db: &DatabaseConnection, run_id: &str) -> Settled {
    match read(db, run_id).await {
        // Reads as runnable although the stamp just said otherwise, which no
        // writer of these rows produces. Not this attempt's to settle either.
        Ok((_, ClaimStep::Run)) => step_aside(
            run_id,
            "the ask reads as unbegun after its stamp was refused".to_string(),
        ),
        Ok((_, step)) => resolve(run_id, step),
        Err(settled) => settled,
    }
}

/// The report of an attempt that is not going to start the pipeline.
fn resolve(run_id: &str, step: ClaimStep) -> Settled {
    match step {
        // `admit` returns the run for this arm; reaching here means the row
        // vanished between the read and the match. Leave it to the next claim.
        ClaimStep::Run => step_aside(run_id, "the ask could not be read back".to_string()),
        ClaimStep::Cancel => Settled::only(TaskOutcome::Cancelled),
        ClaimStep::Parked => step_aside(
            run_id,
            "the ask is at a suspension; it continues from there, not from the top".to_string(),
        ),
        ClaimStep::Live => step_aside(
            run_id,
            "another attempt is executing the ask (its heartbeat is fresh)".to_string(),
        ),
        ClaimStep::Interrupted => {
            tracing::warn!(
                %run_id,
                "agent ask was interrupted by an earlier attempt; closing it, not re-running it"
            );
            Settled::failed(INTERRUPTED_MESSAGE.to_string())
        }
        ClaimStep::Closed(outcome) => {
            tracing::info!(%run_id, "agent ask has already ended; not re-running it");
            Settled::only(*outcome)
        }
    }
}

#[cfg(test)]
mod tests;
