//! Persisting a run as it happens.
//!
//! Per period, not once at the end. A 40-period run is minutes of warehouse
//! queries, so a result that only exists when the loop returns is one an
//! instance death destroys — which is the whole reason this is queued work.

use chrono::Utc;
use entity::{simulation_run_fits, simulation_run_periods, simulation_runs};
use oxy_simulation::{PeriodResult, PolicyKind, ResponseCurve, SimulationSpec};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseConnection,
    EntityTrait, Insert, InsertMany, QueryFilter, UpdateMany,
};
use uuid::Uuid;

/// Everything about a run that is *not* the world it runs.
///
/// A struct rather than five positional arguments, and the split is the point:
/// the world comes from the `.simulation.yml`, and the arm and the draw come
/// from whoever queued it.
#[derive(Debug, Clone, Copy)]
pub struct NewRun {
    pub run_id: Uuid,
    pub workspace_id: Uuid,
    pub revision_id: Option<Uuid>,
    pub policy: PolicyKind,
    pub replicate: u32,
}

/// Write a run row in `queued`, synchronously from the HTTP handler that
/// enqueues it — before the worker fleet has claimed anything.
///
/// This is what makes `GET .../runs/{run_id}` safe to poll the instant
/// `POST .../runs` returns. The row used to appear only when a worker
/// claimed the queued task and called (what is now) [`mark_running`]; a
/// caller that polled in the gap — routinely tens of seconds, since a missed
/// Postgres NOTIFY falls back to the worker's poll interval — saw "no such
/// run" instead of a run that simply hadn't started. Queueing and executing
/// are still two different moments; only the row's existence stopped being
/// one of them.
///
/// Generic over the connection so the handler can write this row in the same
/// transaction as the task that executes it: a row committed with no task
/// behind it would sit at `queued` forever.
pub async fn queue_run(
    db: &impl ConnectionTrait,
    run: NewRun,
    spec: &SimulationSpec,
) -> Result<(), sea_orm::DbErr> {
    queued_insert(run, spec).exec(db).await.map(|_| ())
}

/// The `queued` row. Split from [`queue_run`] so its shape is assertable
/// without a database — see `store/tests.rs`.
fn queued_insert(run: NewRun, spec: &SimulationSpec) -> Insert<simulation_runs::ActiveModel> {
    // One clock reading for both: `started_at` is NOT NULL because the listing
    // index orders on it, so until a worker claims the run it equals
    // `queued_at` and the run reads as zero seconds of runtime rather than as
    // a null. `mark_running` is what moves it.
    let now = Utc::now();
    simulation_runs::Entity::insert(simulation_runs::ActiveModel {
        run_id: Set(run.run_id),
        workspace_id: Set(run.workspace_id),
        revision_id: Set(run.revision_id),
        simulation_name: Set(spec.name.clone()),
        // The wire spelling, not `{:?}`: that renders the explore arm
        // `machineexplore`, which parses back as nothing and reads as a typo in
        // every listing that shows it.
        policy: Set(run.policy.as_str().to_string()),
        // The seed this replicate actually ran, which the payload already
        // substituted into the snapshot — so a run row never has to be read
        // alongside arithmetic to know which world it saw.
        seed: Set(spec.seed as i64),
        replicate: Set(run.replicate as i32),
        status: Set("queued".to_string()),
        spec: Set(serde_json::to_value(spec).unwrap_or(serde_json::Value::Null)),
        truth: Set(None),
        periods_planned: Set(spec.periods as i32),
        periods_done: Set(0),
        queued_at: Set(now.into()),
        started_at: Set(now.into()),
        finished_at: Set(None),
        error: Set(None),
    })
}

/// What a claim write found.
///
/// Two answers rather than `()` because the caller has to tell "this attempt
/// owns the run now" from "some earlier attempt already finished with it", and
/// only the write itself knows which — see [`mark_running`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// The row moved to `running`, and this attempt owns it.
    Claimed,
    /// Nothing moved: the run is already terminal (or its row is gone). The
    /// attempt must not write — the evidence belongs to whoever closed it.
    AlreadyClosed,
}

/// The statuses a run may be claimed *out of*.
///
/// `queued` is the ordinary claim. `running` is the requeue of a run whose
/// lease expired mid-flight, and restamping `started_at` is right there: the
/// attempt that owns the row's runtime is the one now executing. Everything
/// else is terminal.
const CLAIMABLE: [&str; 2] = ["queued", "running"];

/// Flip a claimable row to `running`, once a worker actually claims the task.
///
/// An update, not an insert — [`queue_run`] already wrote the row. Still
/// worth its own status rather than skipping straight to per-period writes:
/// a run that dies between the claim and its first period stays visible as
/// `running` (and eventually stale) rather than looking identical to one
/// still waiting in the queue.
///
/// This is also where `started_at` becomes a start time. `queue_run` stamps
/// it with the enqueue clock only so the column can stay NOT NULL; left
/// there, `finished_at - started_at` would be queue wait plus runtime.
///
/// # Why the precondition is in the `WHERE` and not in the caller
///
/// The same payload executes more than once as a matter of routine: the durable
/// queue's reaper requeues a task whose lease expired
/// (`agentic_runtime::orchestrator::crud::queue::reap_stale_tasks`), and an
/// outcome message can be dropped *after* the run row was already closed out —
/// the transport says so out loud when a peer holds the claim
/// (`TerminalWrite::NotOwned`: "this process just finished work the queue is
/// discarding"). So a second attempt's claim routinely lands on a `done`,
/// `failed` or `cancelled` row.
///
/// Unconditional, this `UPDATE` drove that row back to `running`, and the
/// consequences are not cosmetic: `api::simulation::runs::limits` counts
/// `queued`/`running` against `MAX_IN_FLIGHT_PER_WORKSPACE`, so a resurrected
/// run holds a cap slot until `IN_FLIGHT_MAX_AGE_HOURS` ages it out, and any
/// reader scoped to terminal runs — the profit race — stops seeing evidence
/// that was already written.
///
/// A guarded write rather than a read-then-check in the executor because the
/// two attempts are in different processes: anything short of one statement
/// leaves a window between the read and the write. This is the same rule the
/// queue applies to its own rows — first terminal status wins — and
/// `rows_affected` is how Postgres reports which side of it we landed on.
pub async fn mark_running(db: &DatabaseConnection, run_id: Uuid) -> Result<Claim, sea_orm::DbErr> {
    let moved = running_update(run_id).exec(db).await?;
    Ok(if moved.rows_affected == 0 {
        Claim::AlreadyClosed
    } else {
        Claim::Claimed
    })
}

/// The claim-time update, split out for the same reason as [`queued_insert`].
///
/// `update_many` rather than `Entity::update(..)`: the latter's `WHERE` is the
/// primary key and nothing else, and it reports a miss as
/// `DbErr::RecordNotUpdated` — an error, indistinguishable at the call site
/// from the connection failing. The row count is the answer we need.
fn running_update(run_id: Uuid) -> UpdateMany<simulation_runs::Entity> {
    simulation_runs::Entity::update_many()
        .col_expr(
            simulation_runs::Column::Status,
            Expr::value("running".to_string()),
        )
        .col_expr(
            simulation_runs::Column::StartedAt,
            Expr::value(Utc::now().fixed_offset()),
        )
        .filter(simulation_runs::Column::RunId.eq(run_id))
        // The precondition. Not `status <> 'done'` and friends: an unlisted
        // status a later migration adds would then be silently claimable,
        // whereas naming what may be claimed makes the new status refuse until
        // someone decides it belongs in [`CLAIMABLE`].
        .filter(simulation_runs::Column::Status.is_in(CLAIMABLE))
}

/// Write one period's actions, profit and per-edge scores.
pub async fn record_period(
    db: &DatabaseConnection,
    run_id: Uuid,
    result: &PeriodResult,
) -> Result<(), sea_orm::DbErr> {
    period_upsert(run_id, result).exec(db).await?;

    if !result.fits.is_empty() {
        fits_upsert(run_id, result).exec(db).await?;
    }

    let mut run = simulation_runs::ActiveModel {
        run_id: Set(run_id),
        ..Default::default()
    };
    run.periods_done = Set(result.period as i32);
    run.update(db).await.map(|_| ())
}

/// The period row, as an upsert on `(run_id, period)`.
///
/// An upsert rather than a plain insert because a run's *second attempt* is a
/// normal event: the durable queue requeues a task whose lease expired — the
/// worker-died-mid-run case it exists for — and [`fail_run`] deliberately keeps
/// the dead attempt's periods. The retry restarts at period 1, so a plain
/// insert conflicts on its first write and the raw duplicate-key string lands
/// in the run's `error`, reading as a broken database rather than as a retry.
///
/// Upsert rather than clearing the prior periods in [`mark_running`], the other
/// place this could be fixed: the panel streams a run period by period, and
/// blanking it at claim time would empty the chart for the minutes the retry
/// takes to catch up. Rewriting is safe because both attempts run the same
/// declared world at the same seed — the replaced row is the same evidence, not
/// a second opinion.
fn period_upsert(
    run_id: Uuid,
    result: &PeriodResult,
) -> Insert<simulation_run_periods::ActiveModel> {
    simulation_run_periods::Entity::insert(simulation_run_periods::ActiveModel {
        run_id: Set(run_id),
        period: Set(result.period as i32),
        mean_spend: Set(result.mean_spend),
        realized_profit: Set(result.realized_profit),
        cumulative_profit: Set(result.cumulative_profit),
        actions: Set(serde_json::to_value(&result.actions).unwrap_or(serde_json::Value::Null)),
    })
    .on_conflict(
        sea_orm::sea_query::OnConflict::columns([
            simulation_run_periods::Column::RunId,
            simulation_run_periods::Column::Period,
        ])
        .update_columns([
            simulation_run_periods::Column::MeanSpend,
            simulation_run_periods::Column::RealizedProfit,
            simulation_run_periods::Column::CumulativeProfit,
            simulation_run_periods::Column::Actions,
        ])
        .to_owned(),
    )
}

/// This period's per-edge scores, as an upsert on `(run_id, period, edge)`.
///
/// Same reasoning as [`period_upsert`], and it has to be the same choice: a
/// retry that could rewrite the period row but not its fits would leave a
/// period whose chart and whose badges came from different attempts.
fn fits_upsert(
    run_id: Uuid,
    result: &PeriodResult,
) -> InsertMany<simulation_run_fits::ActiveModel> {
    let fits = result
        .fits
        .iter()
        .map(|f| simulation_run_fits::ActiveModel {
            run_id: Set(run_id),
            period: Set(result.period as i32),
            edge: Set(f.edge.clone()),
            // The basis the fitter chose, so a run records *which* shape
            // it could not read rather than only that it refused.
            form: Set(f.fit.form_name.clone()),
            // Null exactly on a refusal. A 0.0 here would erase the
            // distinction the whole outcome taxonomy turns on.
            coefficient: Set(f.fit.coefficient),
            se: Set(f.fit.coefficient.map(|_| f.fit.se)),
            t_stat: Set(f.fit.coefficient.map(|_| f.fit.t_stat)),
            n: Set(f.fit.n as i32),
            n_panels: Set(f.fit.n_panels as i32),
            refusal: Set(f.fit.refusal.clone()),
            true_local_slope: Set(f.true_local_slope),
            outcome: Set(f.outcome.as_str().to_string()),
        });

    simulation_run_fits::Entity::insert_many(fits).on_conflict(
        sea_orm::sea_query::OnConflict::columns([
            simulation_run_fits::Column::RunId,
            simulation_run_fits::Column::Period,
            simulation_run_fits::Column::Edge,
        ])
        .update_columns([
            simulation_run_fits::Column::Form,
            simulation_run_fits::Column::Coefficient,
            simulation_run_fits::Column::Se,
            simulation_run_fits::Column::TStat,
            simulation_run_fits::Column::N,
            simulation_run_fits::Column::NPanels,
            simulation_run_fits::Column::Refusal,
            simulation_run_fits::Column::TrueLocalSlope,
            simulation_run_fits::Column::Outcome,
        ])
        .to_owned(),
    )
}

/// Close a run, recording the world's true parameters.
///
/// This is the one place truth is written down, and nothing in the loop ever
/// reads it back — see the architecture diagram in the plan.
pub async fn finish_run(
    db: &DatabaseConnection,
    run_id: Uuid,
    truth: ResponseCurve,
) -> Result<(), sea_orm::DbErr> {
    let mut run = simulation_runs::ActiveModel {
        run_id: Set(run_id),
        ..Default::default()
    };
    run.status = Set("done".to_string());
    run.truth = Set(Some(serde_json::json!({
        "theta": truth.theta,
        "scale": truth.scale,
        "anchor_spend": truth.anchor_spend,
        "optimum_spend": truth.optimum_spend,
    })));
    run.finished_at = Set(Some(Utc::now().into()));
    run.update(db).await.map(|_| ())
}

/// Close a run that failed, keeping every period it did produce.
///
/// The partial periods stay: a run that got eight periods in before the
/// warehouse fell over still says something, and deleting them would turn a
/// diagnosable failure into an absence.
pub async fn fail_run(
    db: &DatabaseConnection,
    run_id: Uuid,
    error: String,
) -> Result<(), sea_orm::DbErr> {
    let mut run = simulation_runs::ActiveModel {
        run_id: Set(run_id),
        ..Default::default()
    };
    run.status = Set("failed".to_string());
    run.error = Set(Some(error));
    run.finished_at = Set(Some(Utc::now().into()));
    run.update(db).await.map(|_| ())
}

/// Close a run that a cancel stopped, keeping every period it did produce.
///
/// Same shape as [`fail_run`] and for the same reason — the periods a run got
/// through are evidence, and deleting them would turn a partial answer into an
/// absence. A different status, though: a cancelled run is somebody's decision,
/// and filing it as `failed` would put a reader on the trail of a warehouse
/// that never broke. No `error` for the same reason.
pub async fn cancel_run(db: &DatabaseConnection, run_id: Uuid) -> Result<(), sea_orm::DbErr> {
    let mut run = simulation_runs::ActiveModel {
        run_id: Set(run_id),
        ..Default::default()
    };
    run.status = Set("cancelled".to_string());
    run.finished_at = Set(Some(Utc::now().into()));
    run.update(db).await.map(|_| ())
}

#[cfg(test)]
mod tests;
