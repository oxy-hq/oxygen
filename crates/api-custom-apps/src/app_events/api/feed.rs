//! `GET …/{app}/events/feed?since=&environment=`: what `oxyc events tail`
//! polls. The app's publishes (when the platform relayed each) and its
//! listeners' deliveries (whenever one changed) after `since`, oldest first,
//! at most [`MAX_ITEMS`] a poll, with the cursor the next poll starts from.

use chrono::{DateTime, Duration, Utc};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, QueryResult, Statement,
};
use serde::Serialize;
use uuid::Uuid;

use super::reply::ApiError;

/// The most items one poll returns; the cursor then names the last, so the
/// next poll carries on.
const MAX_ITEMS: usize = 500;
/// How far back a poll naming no `since` reads.
const DEFAULT_LOOKBACK_MINUTES: i64 = 15;

#[derive(Debug, Serialize)]
pub(super) struct FeedReply {
    items: Vec<FeedItem>,
    /// Where the next poll starts: the last item's time, else `since`.
    cursor: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FeedItem {
    /// `published` or `delivery`.
    kind: &'static str,
    at: DateTime<Utc>,
    event_id: Uuid,
    #[serde(rename = "type")]
    event_type: String,
    key: String,
    environment: String,
    /// A delivery's: `<app>/<function>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    listener: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attempt_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
}

const PUBLISHED_SQL: &str = "\
SELECT id AS event_id, type AS event_type, key, environment, relayed_at AS at \
  FROM app_events \
 WHERE source_app_id = $1 AND environment = $2 AND relayed_at > $3 \
 ORDER BY relayed_at LIMIT $4";

const DELIVERIES_SQL: &str = "\
SELECT d.event_id, e.type AS event_type, e.key, d.environment, d.updated_at AS at, \
       d.function_name, d.status, d.attempts, d.last_error \
  FROM app_event_deliveries d JOIN app_events e ON e.id = d.event_id \
 WHERE d.app_id = $1 AND d.environment = $2 AND d.updated_at > $3 \
 ORDER BY d.updated_at LIMIT $4";

/// Where a poll starts: `since`, or the last few minutes.
pub(super) fn since(raw: Option<&str>) -> Result<DateTime<Utc>, ApiError> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => super::reply::time("since", raw),
        None => Ok(Utc::now() - Duration::minutes(DEFAULT_LOOKBACK_MINUTES)),
    }
}

/// The feed of app `app_id` (slug `app_slug`) in `environment` after `since`.
pub(super) async fn feed(
    db: &DatabaseConnection,
    (app_id, app_slug): (Uuid, &str),
    environment: &str,
    since: DateTime<Utc>,
) -> Result<FeedReply, ApiError> {
    let read = |e| ApiError::db("read the feed", e);
    let mut items = Vec::new();
    for row in query(db, PUBLISHED_SQL, app_id, environment, since)
        .await
        .map_err(read)?
    {
        items.push(item(&row, "published", None).map_err(read)?);
    }
    for row in query(db, DELIVERIES_SQL, app_id, environment, since)
        .await
        .map_err(read)?
    {
        items.push(item(&row, "delivery", Some(app_slug)).map_err(read)?);
    }
    items.sort_by_key(|i| i.at);
    items.truncate(MAX_ITEMS);
    let cursor = items.last().map_or(since, |i| i.at);
    Ok(FeedReply { items, cursor })
}

fn item(
    row: &QueryResult,
    kind: &'static str,
    listener_app: Option<&str>,
) -> Result<FeedItem, DbErr> {
    let mut item = FeedItem {
        kind,
        at: row.try_get("", "at")?,
        event_id: row.try_get("", "event_id")?,
        event_type: row.try_get("", "event_type")?,
        key: row.try_get("", "key")?,
        environment: row.try_get("", "environment")?,
        listener: None,
        status: None,
        attempt_count: None,
        last_error: None,
    };
    if let Some(app) = listener_app {
        let function: String = row.try_get("", "function_name")?;
        item.listener = Some(format!("{app}/{function}"));
        item.status = Some(row.try_get("", "status")?);
        item.attempt_count = Some(row.try_get("", "attempts")?);
        item.last_error = row.try_get("", "last_error")?;
    }
    Ok(item)
}

async fn query(
    db: &DatabaseConnection,
    sql: &str,
    app_id: Uuid,
    environment: &str,
    since: DateTime<Utc>,
) -> Result<Vec<QueryResult>, DbErr> {
    db.query_all_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        [
            app_id.into(),
            environment.into(),
            since.into(),
            (MAX_ITEMS as i64).into(),
        ],
    ))
    .await
}
