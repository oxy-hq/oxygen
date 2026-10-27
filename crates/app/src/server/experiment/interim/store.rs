//! The one `experiment_interims` row per run: the enqueue's compare-and-swap, the job's
//! one write, and the read. Never touches `experiment_results`.

use chrono::{DateTime, Duration, NaiveDate, Utc};
use entity::{experiment_interims, experiment_runs};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, EntityTrait,
    QueryFilter, Statement, TransactionTrait,
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::server::experiment::jobs::seed_custom;

pub const INTERIM_KIND: &str = "experiment_interim";
/// A job still marked in flight after this long is presumed lost: a new request replaces it.
pub const INTERIM_STALE_SECS: i64 = 3600;
/// The schedule's cadence: a live run gets a fresh reading once a day.
pub const INTERIM_EVERY_SECS: i64 = 86_400;

/// What one interim job computed.
#[derive(Debug, Clone)]
pub struct Reading {
    pub through: NaiveDate,
    pub report: Value,
    pub effect: Value,
    pub compliance: Value,
    pub series: Option<Value>,
}

pub async fn interim_of(
    db: &impl ConnectionTrait,
    run_id: Uuid,
) -> Result<Option<experiment_interims::Model>, DbErr> {
    experiment_interims::Entity::find_by_id(run_id)
        .one(db)
        .await
}

/// Whether a job is in flight: marked, and not so old that it is presumed lost.
pub fn in_flight(row: &experiment_interims::Model, now: DateTime<Utc>) -> bool {
    row.task_id.is_some()
        && row
            .requested_at
            .is_some_and(|at| now - at.with_timezone(&Utc) < Duration::seconds(INTERIM_STALE_SECS))
}

/// Enqueue one interim job for `run`, unless one is in flight or the last request is
/// newer than `min_gap` (zero on demand, a day on the schedule). A CAS on the row inside the
/// enqueue's transaction, so replicas and double clicks enqueue once. Returns whether this
/// call did.
pub async fn enqueue_interim(
    db: &DatabaseConnection,
    run: &experiment_runs::Model,
    now: DateTime<Utc>,
    min_gap: Duration,
) -> Result<bool, DbErr> {
    let task = Uuid::new_v4();
    let stale = now - Duration::seconds(INTERIM_STALE_SECS);
    let txn = db.begin().await?;
    let won = txn
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO experiment_interims (run_id, task_id, requested_at) VALUES ($1, $2, $3) \
             ON CONFLICT (run_id) DO UPDATE SET task_id = EXCLUDED.task_id, \
               requested_at = EXCLUDED.requested_at \
             WHERE (experiment_interims.task_id IS NULL OR experiment_interims.requested_at <= $4) \
               AND (experiment_interims.requested_at IS NULL OR experiment_interims.requested_at <= $5)",
            [
                run.id.into(),
                task.into(),
                now.fixed_offset().into(),
                stale.fixed_offset().into(),
                (now - min_gap).fixed_offset().into(),
            ],
        ))
        .await?
        .rows_affected()
        == 1;
    if !won {
        txn.rollback().await?;
        return Ok(false);
    }
    seed_custom(&txn, task, INTERIM_KIND, json!({ "run_id": run.id }), run).await?;
    txn.commit().await?;
    Ok(true)
}

/// The job's one write. Only the job the row still names may write it: a job replaced as
/// stale finds its id gone and writes nothing. A failure keeps the previous reading.
pub async fn finish(
    db: &DatabaseConnection,
    run_id: Uuid,
    task: Uuid,
    outcome: &Result<Reading, String>,
    now: DateTime<Utc>,
) -> Result<bool, DbErr> {
    use experiment_interims::Column as C;
    let mut update = experiment_interims::Entity::update_many()
        .col_expr(C::TaskId, Expr::value(Option::<Uuid>::None))
        .filter(C::RunId.eq(run_id))
        .filter(C::TaskId.eq(task));
    update = match outcome {
        Ok(r) => update
            .col_expr(C::ComputedAt, Expr::value(now.fixed_offset()))
            .col_expr(C::Through, Expr::value(r.through))
            .col_expr(C::Report, Expr::value(r.report.clone()))
            .col_expr(C::Effect, Expr::value(r.effect.clone()))
            .col_expr(C::Compliance, Expr::value(r.compliance.clone()))
            .col_expr(C::Series, Expr::value(r.series.clone()))
            .col_expr(C::Error, Expr::value(Option::<String>::None)),
        Err(why) => update.col_expr(C::Error, Expr::value(why.clone())),
    };
    Ok(update.exec(db).await?.rows_affected == 1)
}
