//! The experiment clock (C7). Runs on every node that runs the global driver
//! (`router/recovery.rs`); a run is advanced by exactly one of them per slot,
//! because each claims it by CAS on `next_check_at` — the way `tick_schedules`
//! claims a schedule on `next_run_at`. Database-only: warehouse reads are the
//! `experiment_check` job's. Never errors the caller.

use chrono::{DateTime, Duration, Utc};
use entity::experiment_flips;
use entity::experiment_runs;
use oxy_experiment::Moment;
use oxy_experiment::run::{
    FlipFact, LeverState, RunFacts, RunState, abort_levers, step, step_retrospective,
};
use oxy_experiment::timeline::local_hour_in;
use sea_orm::{DatabaseConnection, DbErr};

use super::dispatch::{self, RunCtx};
use super::estimate::store as estimate_store;
use super::jobs::{self, EstimateState, MAX_ESTIMATE_ATTEMPTS, Retry};
use super::schedule::{RUN_CHECK_INTERVAL_SECS, estimate_owed, next_check_at};
use super::{interim, notify, store};

pub const MAX_RUNS_PER_TICK: u64 = 50;

/// Advances every due run this replica claims; returns how many advanced.
/// Registration is an explicit action (`register::register`), never the tick's.
pub async fn tick_experiments(db: &DatabaseConnection, now: DateTime<Utc>) -> usize {
    let due = match store::due_runs(db, now, MAX_RUNS_PER_TICK).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(target: "experiment", error = %e, "tick: due-run query failed");
            return 0;
        }
    };
    let mut advanced = 0;
    for run in due {
        let claimed_until = now + Duration::seconds(RUN_CHECK_INTERVAL_SECS);
        if !store::claim(db, &run, claimed_until).await {
            continue; // another replica has it
        }
        match advance(db, run.id, now, claimed_until).await {
            Ok(()) => advanced += 1,
            Err(e) => tracing::warn!(
                target: "experiment", run_id = %run.id, error = %e,
                "tick: run not advanced; retried when the claim lapses"
            ),
        }
    }
    advanced
}

async fn advance(
    db: &DatabaseConnection,
    run_id: uuid::Uuid,
    now: DateTime<Utc>,
    claimed_until: DateTime<Utc>,
) -> Result<(), String> {
    let e = |e: DbErr| e.to_string();
    // Re-read after the claim: an abort that landed after the scan is seen now.
    let run = store::run_by_id(db, run_id)
        .await
        .map_err(e)?
        .ok_or("the run vanished")?;
    let c = store::pinned_config(db, &run).await?;
    let prev =
        RunState::parse(&run.state).ok_or_else(|| format!("unknown state {:?}", run.state))?;
    let (run, estimate, retry) = estimate_for_step(db, run, prev, now).await.map_err(e)?;
    let local = local_hour_in(c.design.timezone, now);
    let ctx = RunCtx {
        run: &run,
        c: &c,
        local,
        now,
    };
    dispatch::sync_lever_outcomes(db, &ctx).await.map_err(e)?;
    let done = estimate == EstimateState::Done;
    let facts = facts(db, &run, local, done).await.map_err(e)?;
    // A retrospective run moved no lever and owes none: its own step never flips.
    let t = if run.retrospective {
        step_retrospective(prev, &facts)
    } else {
        step(&c, prev, &facts)
    };
    if t.next == RunState::Aborted && prev != RunState::Aborted {
        dispatch::cancel_undispatched_on(db, run.id)
            .await
            .map_err(e)?;
    }
    dispatch::apply(db, &ctx, &t).await.map_err(e)?;
    if retry == Some(Retry::Exhausted) {
        notify_exhausted(db, &run, &c.owner, &estimate, t.next).await;
    }
    let pending = dispatch::dispatch_pending(db, &ctx).await.map_err(e)?;
    // A check reads back flips and guardrails of a run in progress; a retrospective
    // run has neither, and its estimate pins what a first check would have.
    if !t.next.is_terminal() && !run.retrospective {
        jobs::maybe_enqueue_check(db, &run, now).await.map_err(e)?;
    }
    if t.next == RunState::Live && !run.retrospective {
        schedule_interim(db, &run, now).await;
    }
    let estimate_owed = estimate_owed(&estimate, retry);
    let next = next_check_at(
        &c,
        t.next,
        local,
        now,
        pending.deferred > 0 || estimate_owed,
    );
    // On the way into `aborted`, what the abort does with the levers is fixed: a breach
    // turns them off whatever the owner asked, and a later breach changes nothing.
    let entering_abort = t.next == RunState::Aborted && prev != RunState::Aborted;
    let p = store::Persist {
        prev,
        next: t.next,
        abort_levers: entering_abort.then(|| abort_levers(&facts)),
        local,
        next_check_at: next,
        claimed_until,
        now,
    };
    store::persist(db, run.id, &p).await.map_err(e)?;
    if t.next != prev {
        tracing::info!(
            target: "experiment", run_id = %run.id, experiment = %c.experiment,
            from = prev.as_str(), to = t.next.as_str(), "run advanced"
        );
    }
    Ok(())
}

/// The estimate as `step` sees it, and what the tick decided about a failed one.
///
/// A failed estimate on a `closing` or `aborted` run is released once its
/// backoff has passed and attempts remain: it then reads `None`, so `step`
/// emits `EnqueueEstimate` this tick and `dispatch::apply` enqueues a fresh job
/// through the usual CAS. A run whose result row exists (a lost `Done`) is
/// released past the attempt cap — the re-run only returns `Done` — and is
/// never reported exhausted.
async fn estimate_for_step(
    db: &DatabaseConnection,
    run: experiment_runs::Model,
    state: RunState,
    now: DateTime<Utc>,
) -> Result<(experiment_runs::Model, EstimateState, Option<Retry>), DbErr> {
    let estimate = jobs::estimate_state(db, &run).await?;
    let EstimateState::Failed {
        failed_at,
        attempts,
        ..
    } = &estimate
    else {
        return Ok((run, estimate, None));
    };
    if !matches!(state, RunState::Closing | RunState::Aborted) {
        return Ok((run, estimate, None));
    }
    let sealed = estimate_store::result_exists(db, run.id).await?;
    let retry = jobs::retry_of(*attempts, *failed_at, now, sealed);
    let Some(task) = run.estimate_task_id.filter(|_| retry == Retry::Now) else {
        return Ok((run, estimate, Some(retry)));
    };
    if !jobs::release_failed_estimate(db, run.id, task).await? {
        // Another replica released it: the fresh job is on its way, so the run stays
        // owed (short cadence) rather than settling to the daily recheck.
        return Ok((run, estimate, Some(Retry::Wait)));
    }
    let run = store::run_by_id(db, run.id)
        .await?
        .ok_or_else(|| DbErr::RecordNotFound(format!("run {}", run.id)))?;
    Ok((run, EstimateState::None, Some(Retry::Now)))
}

/// A live run's daily interim reading. Best effort: a failed enqueue never holds the run
/// back, and the next tick tries again.
async fn schedule_interim(
    db: &DatabaseConnection,
    run: &experiment_runs::Model,
    now: DateTime<Utc>,
) {
    let every = Duration::seconds(interim::store::INTERIM_EVERY_SECS);
    if let Err(e) = interim::store::enqueue_interim(db, run, now, every).await {
        tracing::warn!(
            target: "experiment", run_id = %run.id, error = %e,
            "tick: interim reading not enqueued"
        );
    }
}

/// Every attempt failed. Stable wording, so the notice dedupe sends it once.
async fn notify_exhausted(
    db: &DatabaseConnection,
    run: &experiment_runs::Model,
    owner: &str,
    estimate: &EstimateState,
    state: RunState,
) {
    let EstimateState::Failed {
        task_status, error, ..
    } = estimate
    else {
        return;
    };
    let reason = format!(
        "the estimate failed {MAX_ESTIMATE_ATTEMPTS} times (last: {task_status}: {}); the run \
         stays in {}",
        error.as_deref().unwrap_or("no error message"),
        state.as_str()
    );
    notify::notify_owner(db, run, owner, &reason).await;
}

async fn facts(
    db: &DatabaseConnection,
    run: &experiment_runs::Model,
    local: Moment,
    estimate_done: bool,
) -> Result<RunFacts, DbErr> {
    let flips = flip_facts(&store::flips_of(db, run.id).await?);
    Ok(RunFacts {
        now: local,
        flips,
        // A switchback's fleet (Task 10b pins it); a waves design never reads it.
        pinned_units: store::fleet(run),
        last_window_coverage: store::observations(run).coverage,
        closing_since: run.closing_since.map(Moment),
        guardrail_breach: run.guardrail_breach.clone(),
        abort_requested: run.abort_reason.clone(),
        abort_levers: store::abort_levers(run).unwrap_or_default(),
        estimate_done,
    })
}

/// The recorded flips as the state machine reads them. A row whose state does not parse
/// records no lever state and is skipped.
pub(super) fn flip_facts(flips: &[experiment_flips::Model]) -> Vec<FlipFact> {
    flips
        .iter()
        .filter_map(|f| {
            Some(FlipFact {
                state: LeverState::parse(&f.target_state)?,
                dispatched: f.task_id.is_some(),
                cancelled: f.status == "cancelled",
                due_at: Moment(f.due_at),
                unit: f.unit.clone(),
            })
        })
        .collect()
}
