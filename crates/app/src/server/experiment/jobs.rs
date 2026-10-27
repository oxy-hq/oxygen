//! The two background jobs a run enqueues, both `TaskSpec::Custom` Global runs
//! (root task id == run id). Each is guarded by a compare-and-swap on the run
//! row inside the same transaction as the enqueue, so replicas and retried
//! ticks enqueue once: the estimate once per attempt, the check at most hourly.
//! A failed estimate is released by the tick ([`release_failed_estimate`]) and
//! enqueued again, at most [`MAX_ESTIMATE_ATTEMPTS`] runs in all.
//!
//! The `experiment_estimate` executor is `estimate::ExperimentEstimateExecutor`;
//! its payload is [`EstimatePayload`], serialised here and nowhere else.

use agentic_core::delegation::TaskSpec;
use agentic_runtime::orchestrator::crud::queue::TaskScope;
use chrono::{DateTime, Duration, FixedOffset, Utc};
use entity::experiment_runs;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, FromQueryResult, Statement,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

pub const ESTIMATE_KIND: &str = "experiment_estimate";
pub const CHECK_KIND: &str = "experiment_check";
pub const CHECK_TASK_INTERVAL_SECS: i64 = 3600;
/// A failed estimate is run again by the tick, up to this many runs in all.
pub const MAX_ESTIMATE_ATTEMPTS: i64 = 3;
/// How long after a failure the tick waits before running the estimate again.
pub const ESTIMATE_RETRY_AFTER_SECS: i64 = 3600;

/// What an `experiment_estimate` task carries (C4): the run, and nothing else.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EstimatePayload {
    pub run_id: Uuid,
}

/// Where the run's `experiment_estimate` job stands, read from its agentic run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum EstimateState {
    /// Never enqueued (the run has not reached the estimate yet).
    None,
    /// Enqueued, or claimed and running.
    Running,
    /// Done: PR 5's executor completes only after writing the result.
    Done,
    /// Ended without a result: `task_status` is `failed`, `cancelled` or
    /// `timed_out`, at `failed_at`. The run stays where it is; the tick runs the
    /// estimate again after [`ESTIMATE_RETRY_AFTER_SECS`] while `attempts` (every
    /// estimate run enqueued for this run, this one included) is below
    /// [`MAX_ESTIMATE_ATTEMPTS`].
    Failed {
        task_status: String,
        error: Option<String>,
        failed_at: DateTime<Utc>,
        attempts: i64,
    },
}

/// What the tick does with a failed estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retry {
    /// Release it now: the backoff has passed and attempts remain.
    Now,
    /// Attempts remain; the backoff has not passed yet.
    Wait,
    /// Every attempt failed: tell the owner, run nothing more.
    Exhausted,
}

/// When a failed estimate runs again. Pure: the tick supplies the facts.
///
/// `result_exists`: the result row was written although the job reads failed
/// (a lost `Done`). The re-run computes nothing and only returns `Done`, so it
/// is not capped by [`MAX_ESTIMATE_ATTEMPTS`]; the backoff still applies.
pub fn retry_of(
    attempts: i64,
    failed_at: DateTime<Utc>,
    now: DateTime<Utc>,
    result_exists: bool,
) -> Retry {
    if !result_exists && attempts >= MAX_ESTIMATE_ATTEMPTS {
        Retry::Exhausted
    } else if now - failed_at >= Duration::seconds(ESTIMATE_RETRY_AFTER_SECS) {
        Retry::Now
    } else {
        Retry::Wait
    }
}

#[derive(FromQueryResult)]
struct EstimateRow {
    task_status: Option<String>,
    error_message: Option<String>,
    updated_at: DateTime<FixedOffset>,
}

#[derive(FromQueryResult)]
struct Count {
    n: i64,
}

/// How many estimate runs were ever enqueued for the run: one `agentic_runs`
/// row each, written by [`seed_custom`] with the run id in its `metadata`.
pub async fn estimate_attempts(db: &DatabaseConnection, run_id: Uuid) -> Result<i64, DbErr> {
    let row = Count::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT COUNT(*) AS n FROM agentic_runs \
         WHERE source_type = $1 AND metadata->>'run_id' = $2",
        [ESTIMATE_KIND.into(), run_id.to_string().into()],
    ))
    .one(db)
    .await?;
    Ok(row.map_or(0, |r| r.n))
}

pub async fn estimate_state(
    db: &DatabaseConnection,
    run: &experiment_runs::Model,
) -> Result<EstimateState, DbErr> {
    let Some(task) = run.estimate_task_id else {
        return Ok(EstimateState::None);
    };
    let row = EstimateRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT task_status, error_message, updated_at FROM agentic_runs WHERE id = $1",
        [task.to_string().into()],
    ))
    .one(db)
    .await?;
    // No agentic run row (not yet visible, or removed) reads as running: a
    // missing job is not a failed one. A lost job is not detected yet (parked).
    let Some(row) = row else {
        return Ok(EstimateState::Running);
    };
    Ok(match row.task_status.as_deref() {
        Some("done") => EstimateState::Done,
        Some(s @ ("failed" | "cancelled" | "timed_out")) => EstimateState::Failed {
            task_status: s.to_string(),
            error: row.error_message.filter(|e| !e.is_empty()),
            failed_at: row.updated_at.with_timezone(&Utc),
            attempts: estimate_attempts(db, run.id).await?,
        },
        _ => EstimateState::Running,
    })
}

/// Enqueue the run's estimate job once. Returns whether this call did.
pub async fn enqueue_estimate(
    db: &DatabaseConnection,
    run: &experiment_runs::Model,
) -> Result<bool, DbErr> {
    let task = Uuid::new_v4();
    let txn = db.begin().await?;
    let won = txn
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE experiment_runs SET estimate_task_id = $1 \
             WHERE id = $2 AND estimate_task_id IS NULL",
            [task.into(), run.id.into()],
        ))
        .await?
        .rows_affected()
        == 1;
    if !won {
        txn.rollback().await?;
        return Ok(false);
    }
    let payload = serde_json::to_value(EstimatePayload { run_id: run.id })
        .map_err(|e| DbErr::Custom(e.to_string()))?;
    seed_custom(&txn, task, ESTIMATE_KIND, payload, run).await?;
    txn.commit().await?;
    Ok(true)
}

/// Clear the run's failed estimate task so the next `EnqueueEstimate` enqueues
/// a fresh one. A CAS on the failed id: of two replicas only one releases, and a
/// run that has already re-enqueued is left alone. Returns whether this call did.
pub async fn release_failed_estimate(
    db: &DatabaseConnection,
    run_id: Uuid,
    failed_task: Uuid,
) -> Result<bool, DbErr> {
    let res = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE experiment_runs SET estimate_task_id = NULL \
             WHERE id = $1 AND estimate_task_id = $2",
            [run_id.into(), failed_task.into()],
        ))
        .await?;
    Ok(res.rows_affected() == 1)
}

/// Enqueue a check job unless one was enqueued within the last hour. Returns
/// whether this call did.
pub async fn maybe_enqueue_check(
    db: &DatabaseConnection,
    run: &experiment_runs::Model,
    now: DateTime<Utc>,
) -> Result<bool, DbErr> {
    let task = Uuid::new_v4();
    let stale = now - Duration::seconds(CHECK_TASK_INTERVAL_SECS);
    let txn = db.begin().await?;
    let won = txn
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE experiment_runs SET check_enqueued_at = $1 \
             WHERE id = $2 AND (check_enqueued_at IS NULL OR check_enqueued_at <= $3)",
            [
                now.fixed_offset().into(),
                run.id.into(),
                stale.fixed_offset().into(),
            ],
        ))
        .await?
        .rows_affected()
        == 1;
    if !won {
        txn.rollback().await?;
        return Ok(false);
    }
    seed_custom(&txn, task, CHECK_KIND, json!({ "run_id": run.id }), run).await?;
    txn.commit().await?;
    Ok(true)
}

pub(crate) async fn seed_custom(
    txn: &impl ConnectionTrait,
    task: Uuid,
    kind: &str,
    payload: serde_json::Value,
    run: &experiment_runs::Model,
) -> Result<(), DbErr> {
    let id = task.to_string();
    let name = super::store::experiment_name(txn, run).await?;
    agentic_runtime::crud::insert_run(
        txn,
        &id,
        &format!("experiment {name} · {kind}"),
        None,
        kind,
        Some(json!({ "workspace_id": run.workspace_id, "experiment": name, "run_id": run.id })),
        run.workspace_id,
    )
    .await?;
    let spec = TaskSpec::Custom {
        kind: kind.to_string(),
        payload,
    };
    agentic_runtime::crud::enqueue_task(txn, &id, &id, None, &spec, None, TaskScope::Global).await
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone, Utc};

    use super::*;

    #[test]
    fn a_failed_estimate_waits_out_the_backoff_then_runs_again_until_the_attempts_are_spent() {
        let failed = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
        let after = failed + Duration::seconds(ESTIMATE_RETRY_AFTER_SECS);
        assert_eq!(
            retry_of(1, failed, after - Duration::seconds(1), false),
            Retry::Wait
        );
        assert_eq!(retry_of(1, failed, after, false), Retry::Now);
        assert_eq!(
            retry_of(MAX_ESTIMATE_ATTEMPTS - 1, failed, after, false),
            Retry::Now
        );
        assert_eq!(
            retry_of(MAX_ESTIMATE_ATTEMPTS, failed, after, false),
            Retry::Exhausted
        );
        assert_eq!(
            retry_of(MAX_ESTIMATE_ATTEMPTS, failed, failed, false),
            Retry::Exhausted
        );
    }

    #[test]
    fn a_failed_estimate_whose_result_exists_waits_out_the_backoff_but_never_the_cap() {
        let failed = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
        let after = failed + Duration::seconds(ESTIMATE_RETRY_AFTER_SECS);
        let spent = MAX_ESTIMATE_ATTEMPTS + 2;
        assert_eq!(
            retry_of(spent, failed, after - Duration::seconds(1), true),
            Retry::Wait
        );
        assert_eq!(retry_of(spent, failed, after, true), Retry::Now);
    }
}
