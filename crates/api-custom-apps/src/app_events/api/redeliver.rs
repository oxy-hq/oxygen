//! Redelivery: a `dead`, `expired` or `held` delivery queued again, one
//! event's or in bulk.
//!
//! **The same delivery, the same event.** The row keeps its id and every past
//! attempt; it goes back to `pending` with a new task, and its attempt budget
//! restarts where it stands (`app_event_delivery_budgets`), so a dead delivery
//! gets its listener's full `maxAttempts` again rather than dying at once
//! (`deliveries::begin`). The tasks are spread out per listener
//! ([`super::queue`]). A dry run counts and writes nothing.

use chrono::{DateTime, Utc};
use entity::app_event_deliveries as deliveries;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, DbErr, EntityTrait,
    Statement, TransactionTrait,
};
use uuid::Uuid;

use super::queue::Queuer;
use super::reply::{ApiError, Counted};

/// What may be redelivered.
pub(super) const REDELIVERABLE: [&str; 3] = ["dead", "expired", "held"];
/// The most deliveries one call queues; `remaining` counts the rest.
const BATCH: i64 = 1_000;

/// Which deliveries a redelivery takes.
pub(super) struct Selection {
    pub org_id: Uuid,
    pub statuses: Vec<String>,
    /// One event's, or every event's.
    pub event_id: Option<Uuid>,
    /// One listener's, `(app id, function)`, or every listener's.
    pub listener: Option<(Uuid, String)>,
    /// Only those that reached their status since.
    pub since: Option<DateTime<Utc>>,
}

const MATCHING: &str = "\
  FROM app_event_deliveries d JOIN app_events e ON e.id = d.event_id \
 WHERE e.org_id = $1 AND d.status = ANY($2) \
   AND ($3::uuid IS NULL OR d.event_id = $3) \
   AND ($4::uuid IS NULL OR (d.app_id = $4 AND d.function_name = $5)) \
   AND ($6::timestamptz IS NULL OR d.updated_at >= $6)";

/// Redeliver what `selection` names, or count it on a dry run.
pub(super) async fn redeliver(
    db: &DatabaseConnection,
    selection: &Selection,
    dry_run: bool,
) -> Result<Counted, ApiError> {
    let total = matching(db, selection)
        .await
        .map_err(|e| ApiError::db("count deliveries", e))?;
    let batch = total.min(BATCH as u64);
    if dry_run {
        return Ok(Counted {
            count: batch,
            remaining: total - batch,
            dry_run,
        });
    }
    let queued = queue_batch(db, selection)
        .await
        .map_err(|e| ApiError::db("redeliver", e))?;
    Ok(Counted {
        count: queued,
        remaining: total.saturating_sub(queued),
        dry_run,
    })
}

async fn matching(db: &DatabaseConnection, selection: &Selection) -> Result<u64, DbErr> {
    let row = db
        .query_one_raw(statement(
            &format!("SELECT count(*) AS n {MATCHING}"),
            selection,
            None,
        ))
        .await?;
    let n: i64 = row.map(|r| r.try_get("", "n")).transpose()?.unwrap_or(0);
    Ok(n as u64)
}

/// Lock a batch, queue each one's task and put it back to `pending`, in one
/// transaction.
async fn queue_batch(db: &DatabaseConnection, selection: &Selection) -> Result<u64, DbErr> {
    let txn = db.begin().await?;
    let sql = format!(
        "SELECT d.id, e.type AS event_type {MATCHING} ORDER BY d.created_at, d.id LIMIT $7 \
         FOR UPDATE OF d"
    );
    let rows = txn
        .query_all_raw(statement(&sql, selection, Some(BATCH)))
        .await?;
    let mut queuer = Queuer::new(db, selection.org_id);
    let mut queued = 0;
    for row in rows {
        let id: Uuid = row.try_get("", "id")?;
        let event_type: String = row.try_get("", "event_type")?;
        let Some(delivery) = deliveries::Entity::find_by_id(id).one(&txn).await? else {
            continue;
        };
        let Some(run_id) = queuer.queue(&txn, &delivery, &event_type).await? else {
            continue;
        };
        reopen(&txn, &delivery, &run_id).await?;
        queued += 1;
    }
    txn.commit().await?;
    Ok(queued)
}

/// `delivery` back to `pending` on task `run_id`, its budget restarting from
/// the attempts it has made.
async fn reopen(
    txn: &DatabaseTransaction,
    delivery: &deliveries::Model,
    run_id: &str,
) -> Result<(), DbErr> {
    txn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE app_event_deliveries SET status = 'pending', run_id = $2, updated_at = now() \
          WHERE id = $1",
        [delivery.id.into(), run_id.into()],
    ))
    .await?;
    txn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO app_event_delivery_budgets (delivery_id, attempts_before, restarted_at) \
         VALUES ($1, $2, now()) \
         ON CONFLICT (delivery_id) DO UPDATE \
            SET attempts_before = EXCLUDED.attempts_before, restarted_at = EXCLUDED.restarted_at",
        [delivery.id.into(), delivery.attempts.into()],
    ))
    .await?;
    Ok(())
}

fn statement(sql: &str, s: &Selection, limit: Option<i64>) -> Statement {
    let (app, function) = match &s.listener {
        Some((app, function)) => (Some(*app), Some(function.clone())),
        None => (None, None),
    };
    let mut values: Vec<sea_orm::Value> = vec![
        s.org_id.into(),
        s.statuses.clone().into(),
        s.event_id.into(),
        app.into(),
        function.into(),
        s.since.into(),
    ];
    if let Some(limit) = limit {
        values.push(limit.into());
    }
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}
