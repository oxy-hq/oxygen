//! Replay (`POST …/events/replay`): a seek, for a listener that joined late.
//! A new delivery to the listener, queued and spread out, for each event of
//! the type in the environment published in the window that it has none of —
//! **retained events only**: the window starts no earlier than retention
//! does, and an event whose body is no longer in the org's store is skipped
//! and counted (`skippedBodyGone`). Each candidate's body is read to know it
//! is kept, so a call takes at most [`BATCH`]; a store that cannot be reached
//! answers `503` with nothing written.

use chrono::{DateTime, Duration, Utc};
use entity::{app_event_deliveries as deliveries, app_events};
use oxy_oltp::events::{EventsError, read_body};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, EntityTrait,
    QueryFilter, QueryResult, Statement, TransactionTrait,
};
use serde::Serialize;
use uuid::Uuid;

use super::queue::Queuer;
use super::reply::ApiError;
use crate::app_events::relay::relayed_store;
use crate::app_events::retention::RETENTION_DAYS;

/// The most events one call replays.
const BATCH: i64 = 500;

/// What a replay asks for, checked.
pub(super) struct Replay {
    pub org_id: Uuid,
    pub event_type: String,
    pub environment: String,
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
    /// The listener: its app's id and its function.
    pub listener: (Uuid, String),
    pub dry_run: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReplayReply {
    /// Deliveries created, or that would be on a dry run.
    count: u64,
    /// Events in the window whose body is no longer kept.
    skipped_body_gone: u64,
    /// Events in the window the listener already has a delivery of.
    skipped_existing: u64,
    /// Candidates past this call's batch: run it again for them.
    remaining: u64,
    dry_run: bool,
}

const CANDIDATES: &str = "\
  FROM app_events e \
 WHERE e.org_id = $1 AND e.type = $2 AND e.environment = $3 AND e.relay_error IS NULL \
   AND e.published_at >= $4 AND e.published_at <= $5";

const NOT_DELIVERED: &str = "NOT EXISTS (SELECT 1 FROM app_event_deliveries d \
   WHERE d.event_id = e.id AND d.app_id = $6 AND d.function_name = $7)";

pub(super) async fn replay(db: &DatabaseConnection, r: &Replay) -> Result<ReplayReply, ApiError> {
    let read = |e| ApiError::db("read the events to replay", e);
    let existing = count(db, r, &format!("{CANDIDATES} AND NOT {NOT_DELIVERED}"))
        .await
        .map_err(read)?;
    let open = count(db, r, &format!("{CANDIDATES} AND {NOT_DELIVERED}"))
        .await
        .map_err(read)?;
    let sql = format!(
        "SELECT e.id {CANDIDATES} AND {NOT_DELIVERED} ORDER BY e.published_at, e.id LIMIT $8"
    );
    let ids: Vec<Uuid> = db
        .query_all_raw(statement(&sql, r, Some(BATCH)))
        .await
        .map_err(read)?
        .iter()
        .map(|row| row.try_get("", "id"))
        .collect::<Result<_, DbErr>>()
        .map_err(read)?;
    let events = app_events::Entity::find()
        .filter(app_events::Column::Id.is_in(ids.clone()))
        .all(db)
        .await
        .map_err(read)?;
    let (kept, gone) = kept_bodies(db, events).await?;
    let count = if r.dry_run {
        kept.len() as u64
    } else {
        create(db, r, &kept)
            .await
            .map_err(|e| ApiError::db("replay", e))?
    };
    Ok(ReplayReply {
        count,
        skipped_body_gone: gone,
        skipped_existing: existing,
        remaining: open.saturating_sub(ids.len() as u64),
        dry_run: r.dry_run,
    })
}

/// The window's floor: `since`, but no earlier than retention keeps events.
pub(super) fn floor(since: DateTime<Utc>, now: DateTime<Utc>) -> DateTime<Utc> {
    since.max(now - Duration::days(RETENTION_DAYS))
}

/// Of `events`, those whose body the org's store still keeps (publish order),
/// and how many it does not.
async fn kept_bodies(
    db: &DatabaseConnection,
    mut events: Vec<app_events::Model>,
) -> Result<(Vec<app_events::Model>, u64), ApiError> {
    events.sort_by(|a, b| (a.published_at, a.id).cmp(&(b.published_at, b.id)));
    let mut kept = Vec::new();
    let mut gone = 0;
    for event in events {
        let Some(target) = relayed_store(&event.environment) else {
            gone += 1;
            continue;
        };
        match read_body(db, event.org_id, &target, &event.writer_role, event.id).await {
            Ok(Some(_)) => kept.push(event),
            // Gone, never readable, or no store (or branch) to hold it: no body.
            Ok(None)
            | Err(
                EventsError::Undecodable(_)
                | EventsError::NotProvisioned { .. }
                | EventsError::NoBranch { .. },
            ) => gone += 1,
            Err(e @ EventsError::Store(_)) => {
                return Err(ApiError::StoreUnreachable(e.to_string()));
            }
        }
    }
    Ok((kept, gone))
}

/// A pending delivery of each of `events` to the listener, its task queued,
/// in one transaction; how many were created (one a concurrent replay made
/// first is left alone).
async fn create(
    db: &DatabaseConnection,
    r: &Replay,
    events: &[app_events::Model],
) -> Result<u64, DbErr> {
    let txn = db.begin().await?;
    let mut queuer = Queuer::new(db, r.org_id);
    let mut created = 0;
    for event in events {
        let id = Uuid::new_v4();
        let inserted = txn
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "INSERT INTO app_event_deliveries \
                   (id, event_id, app_id, function_name, environment, status, attempts) \
                 VALUES ($1, $2, $3, $4, $5, 'pending', 0) \
                 ON CONFLICT (event_id, app_id, function_name) DO NOTHING",
                [
                    id.into(),
                    event.id.into(),
                    r.listener.0.into(),
                    r.listener.1.clone().into(),
                    event.environment.clone().into(),
                ],
            ))
            .await?
            .rows_affected();
        if inserted != 1 {
            continue;
        }
        let Some(delivery) = deliveries::Entity::find_by_id(id).one(&txn).await? else {
            continue;
        };
        let Some(run_id) = queuer.queue(&txn, &delivery, &event.r#type).await? else {
            continue;
        };
        txn.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE app_event_deliveries SET run_id = $2 WHERE id = $1",
            [id.into(), run_id.into()],
        ))
        .await?;
        created += 1;
    }
    txn.commit().await?;
    Ok(created)
}

async fn count(db: &DatabaseConnection, r: &Replay, from: &str) -> Result<u64, DbErr> {
    let row: Option<QueryResult> = db
        .query_one_raw(statement(&format!("SELECT count(*) AS n {from}"), r, None))
        .await?;
    let n: i64 = row
        .map(|row| row.try_get("", "n"))
        .transpose()?
        .unwrap_or(0);
    Ok(n as u64)
}

fn statement(sql: &str, r: &Replay, limit: Option<i64>) -> Statement {
    let mut values: Vec<sea_orm::Value> = vec![
        r.org_id.into(),
        r.event_type.clone().into(),
        r.environment.clone().into(),
        r.since.into(),
        r.until.into(),
        r.listener.0.into(),
        r.listener.1.clone().into(),
    ];
    if let Some(limit) = limit {
        values.push(limit.into());
    }
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}
