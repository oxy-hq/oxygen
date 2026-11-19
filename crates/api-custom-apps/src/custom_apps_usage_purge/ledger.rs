//! The ledger of purges a deleted app still owes (`custom_app_usage_purges`).
//!
//! The queue row of a purge task is not the record of the debt: queue
//! retention prunes completed rows after 7 days and failed ones after 30, so a
//! purge that outlasted an Airhouse outage would have left no trace. This row
//! is written in the transaction that deletes the app and is never pruned while
//! the purge is owed.
//!
//! **Retry schedule.** A failed run schedules the next at
//! `now + min(5 min x 2^attempts, 6 h)`, `attempts` counting the failure just
//! recorded: 10 min, 20, 40, 80, 160, 320, then every 6 h. After
//! [`MAX_ATTEMPTS`] (24) failures the purge is abandoned, which is about 4.7
//! days after the first failure (630 min of ramp, then 17 x 6 h): long enough to
//! ride out a multi-day Airhouse outage, and far inside the 30-day deletion
//! commitment, so a person has weeks to act on the one `error!` it logs.
//!
//! **Abandoned rows are the visible debt.** They are never pruned. An operator
//! retries one by clearing `abandoned_at` and resetting `attempts` to 0 (and
//! `next_attempt_at` to now); the next sweep queues it. Completed rows are
//! pruned after [`COMPLETED_RETENTION`].

use std::time::Duration;

use chrono::{DateTime, Utc};
use entity::custom_app_usage_purges as purges;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseBackend,
    DatabaseConnection, DbErr, EntityTrait, QueryFilter, QuerySelect, Statement, TransactionTrait,
};
use uuid::Uuid;

use super::{USAGE_PURGE_KIND, UsagePurgeTask};

/// Failed runs after which a purge is abandoned.
pub const MAX_ATTEMPTS: i32 = 24;
/// The first retry delay, doubled per failure.
const BACKOFF_BASE: Duration = Duration::from_secs(5 * 60);
/// The longest wait between runs.
const BACKOFF_CAP: Duration = Duration::from_secs(6 * 3600);
/// How long a completed row is kept.
pub const COMPLETED_RETENTION: Duration = Duration::from_secs(90 * 24 * 3600);
/// `last_error` is cut to this many characters.
const ERROR_MAX_CHARS: usize = 500;

/// The wait after the `attempts`-th failure.
pub fn backoff(attempts: i32) -> Duration {
    let doublings = attempts.clamp(0, 30) as u32;
    BACKOFF_BASE
        .saturating_mul(1u32 << doublings)
        .min(BACKOFF_CAP)
}

/// What recording a failure did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// Another run is scheduled for `next_attempt_at`.
    Retry {
        attempts: i32,
        next_attempt_at: DateTime<Utc>,
    },
    /// This failure was the last: the purge is parked and stays owed.
    /// Returned once; the caller logs it.
    Abandoned { attempts: i32 },
    /// The purge is already completed or abandoned; nothing changed.
    Settled,
}

/// Record that `task`'s app owes a purge, on `conn`: the transaction that
/// deletes the app's row. Due immediately, since its first run is queued with it.
pub async fn record_owed<C: ConnectionTrait>(
    conn: &C,
    task: &UsagePurgeTask,
    org_id: Uuid,
    now: DateTime<Utc>,
) -> Result<(), DbErr> {
    purges::ActiveModel {
        app_id: Set(task.app_id),
        workspace_id: Set(task.workspace_id),
        org_id: Set(org_id),
        created_at: Set(now.into()),
        attempts: Set(0),
        next_attempt_at: Set(now.into()),
        last_error: Set(None),
        completed_at: Set(None),
        abandoned_at: Set(None),
    }
    .insert(conn)
    .await
    .map(|_| ())
}

/// Close the purge: Airhouse has no facts of the app left. A purge already
/// closed stays as it was.
pub async fn complete(
    db: &DatabaseConnection,
    app_id: Uuid,
    now: DateTime<Utc>,
) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE custom_app_usage_purges SET completed_at = $2, last_error = NULL \
          WHERE app_id = $1 AND completed_at IS NULL",
        [app_id.into(), now.into()],
    ))
    .await
    .map(|_| ())
}

/// Record a failed run: count it, schedule the next, or park the purge when
/// that was the last allowed.
pub async fn fail(
    db: &DatabaseConnection,
    app_id: Uuid,
    error: &str,
    now: DateTime<Utc>,
) -> Result<Failure, DbErr> {
    let txn = db.begin().await?;
    let row = purges::Entity::find_by_id(app_id)
        .lock_exclusive()
        .one(&txn)
        .await?;
    let Some(row) = row.filter(|r| r.completed_at.is_none() && r.abandoned_at.is_none()) else {
        return Ok(Failure::Settled);
    };
    let attempts = row.attempts + 1;
    let next = now + backoff(attempts);
    let abandoned = attempts >= MAX_ATTEMPTS;
    let mut active: purges::ActiveModel = row.into();
    active.attempts = Set(attempts);
    active.next_attempt_at = Set(next.into());
    active.last_error = Set(Some(error.chars().take(ERROR_MAX_CHARS).collect()));
    active.abandoned_at = Set(abandoned.then(|| now.into()));
    active.update(&txn).await?;
    txn.commit().await?;
    Ok(if abandoned {
        Failure::Abandoned { attempts }
    } else {
        Failure::Retry {
            attempts,
            next_attempt_at: next,
        }
    })
}

/// Purges owed and due at `now` whose app has no run waiting or running,
/// soonest first.
const DUE_SQL: &str = "\
    SELECT p.app_id, p.workspace_id FROM custom_app_usage_purges p \
     WHERE p.completed_at IS NULL AND p.abandoned_at IS NULL AND p.next_attempt_at <= $1 \
       AND NOT EXISTS ( \
           SELECT 1 FROM agentic_task_queue u \
            WHERE u.spec->>'kind' = $2 \
              AND u.spec->'payload'->>'app_id' = p.app_id::text \
              AND u.queue_status IN ('queued', 'claimed')) \
     ORDER BY p.next_attempt_at, p.app_id LIMIT $3";

/// The purges due at `now` that need a new run.
pub async fn due(
    db: &DatabaseConnection,
    now: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<UsagePurgeTask>, DbErr> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            DUE_SQL,
            [now.into(), USAGE_PURGE_KIND.into(), limit.into()],
        ))
        .await?;
    rows.iter()
        .map(|r| {
            Ok(UsagePurgeTask {
                app_id: r.try_get("", "app_id")?,
                workspace_id: r.try_get("", "workspace_id")?,
            })
        })
        .collect()
}

/// Delete completed rows older than [`COMPLETED_RETENTION`]. Abandoned and owed
/// rows are never deleted.
pub async fn prune_completed(db: &DatabaseConnection, now: DateTime<Utc>) -> Result<u64, DbErr> {
    let cutoff = now - COMPLETED_RETENTION;
    purges::Entity::delete_many()
        .filter(purges::Column::CompletedAt.lt(cutoff))
        .exec(db)
        .await
        .map(|r| r.rows_affected)
}
