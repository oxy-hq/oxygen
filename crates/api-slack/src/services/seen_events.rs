//! Deduplication service for Slack events.
//!
//! Slack guarantees at-least-once delivery — a single event_id may be
//! delivered multiple times if the initial delivery timed out. This service
//! stores seen event IDs in the DB and provides a `claim()` helper that
//! returns `true` only the first time a given ID is seen, preventing
//! duplicate agent runs on retried deliveries.

use chrono::{Duration, Utc};
use entity::prelude::SlackSeenEvents;
use entity::slack_seen_events;
use oxy::database::client::establish_connection;
use oxy_shared::errors::OxyError;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter, SqlErr};

pub struct SeenEventsService;

impl SeenEventsService {
    /// Attempt to claim an event_id.
    ///
    /// Returns `Ok(true)` if this is the first time the event has been seen
    /// (caller should process the event). Returns `Ok(false)` if it is a
    /// duplicate (caller should skip it).
    pub async fn claim(event_id: &str) -> Result<bool, OxyError> {
        let conn = establish_connection().await?;
        let result = slack_seen_events::ActiveModel {
            event_id: ActiveValue::Set(event_id.to_string()),
            received_at: ActiveValue::NotSet,
        }
        .insert(&conn)
        .await;

        match result {
            Ok(_) => Ok(true),
            Err(ref e) => {
                if matches!(e.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) {
                    Ok(false)
                } else {
                    Err(OxyError::DBError(result.unwrap_err().to_string()))
                }
            }
        }
    }

    /// Delete events older than `older_than`. Safe to call periodically.
    pub async fn sweep(older_than: Duration) -> Result<u64, OxyError> {
        let cutoff: sea_orm::prelude::DateTimeWithTimeZone = (Utc::now() - older_than).into();
        let conn = establish_connection().await?;
        let res = SlackSeenEvents::delete_many()
            .filter(slack_seen_events::Column::ReceivedAt.lt(cutoff))
            .exec(&conn)
            .await
            .map_err(|e| OxyError::DBError(e.to_string()))?;
        Ok(res.rows_affected)
    }
}
