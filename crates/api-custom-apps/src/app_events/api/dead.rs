//! `GET …/{app}/events/dead?environment=`: the app's dead deliveries, grouped
//! by error signature (the outcome plus the first line of the error, both its
//! last attempt's), so ten dead letters with one cause read as one problem.
//! Each carries every attempt. At most [`MAX_DEAD`], newest first.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use entity::{app_event_deliveries as deliveries, app_events};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::Serialize;
use uuid::Uuid;

use super::reply::{ApiError, first_line};
use super::show::{AttemptView, attempts_of};

/// The most dead deliveries one reply lists; `truncated` says there are more.
const MAX_DEAD: u64 = 500;

#[derive(Debug, Serialize)]
pub(super) struct DeadReply {
    environment: String,
    groups: Vec<DeadGroup>,
    truncated: bool,
}

#[derive(Debug, Serialize)]
pub(super) struct DeadGroup {
    /// `<outcome>: <first line of the error>`.
    signature: String,
    outcome: String,
    error: String,
    count: usize,
    deliveries: Vec<DeadDelivery>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeadDelivery {
    id: Uuid,
    event_id: Uuid,
    #[serde(rename = "type")]
    event_type: String,
    key: String,
    listener: String,
    attempt_count: i32,
    last_error: Option<String>,
    died_at: DateTime<Utc>,
    attempts: Vec<AttemptView>,
}

/// The dead deliveries of app `app_id` (slug `app_slug`) in `environment`.
pub(super) async fn dead(
    db: &DatabaseConnection,
    (app_id, app_slug): (Uuid, &str),
    environment: &str,
) -> Result<DeadReply, ApiError> {
    let read = |e| ApiError::db("read the dead deliveries", e);
    let mut rows = deliveries::Entity::find()
        .filter(deliveries::Column::AppId.eq(app_id))
        .filter(deliveries::Column::Environment.eq(environment))
        .filter(deliveries::Column::Status.eq("dead"))
        .order_by_desc(deliveries::Column::UpdatedAt)
        .limit(MAX_DEAD + 1)
        .all(db)
        .await
        .map_err(read)?;
    let truncated = rows.len() as u64 > MAX_DEAD;
    rows.truncate(MAX_DEAD as usize);
    let events: HashMap<Uuid, app_events::Model> = app_events::Entity::find()
        .filter(app_events::Column::Id.is_in(rows.iter().map(|d| d.event_id).collect::<Vec<_>>()))
        .all(db)
        .await
        .map_err(read)?
        .into_iter()
        .map(|e| (e.id, e))
        .collect();
    let mut tried = attempts_of(db, rows.iter().map(|d| d.id).collect())
        .await
        .map_err(read)?;
    let mut groups = Groups::default();
    for d in rows {
        let Some(event) = events.get(&d.event_id) else {
            continue;
        };
        let attempts = tried.remove(&d.id).unwrap_or_default();
        groups.add(dead_delivery(d, event, app_slug, attempts));
    }
    Ok(DeadReply {
        environment: environment.to_string(),
        groups: groups.sorted(),
        truncated,
    })
}

fn dead_delivery(
    d: deliveries::Model,
    event: &app_events::Model,
    app_slug: &str,
    attempts: Vec<AttemptView>,
) -> DeadDelivery {
    DeadDelivery {
        id: d.id,
        event_id: d.event_id,
        event_type: event.r#type.clone(),
        key: event.key.clone(),
        listener: format!("{app_slug}/{}", d.function_name),
        attempt_count: d.attempts,
        last_error: d.last_error,
        died_at: d.updated_at.with_timezone(&Utc),
        attempts,
    }
}

/// Dead deliveries by signature, in the order each signature was first seen.
#[derive(Default)]
struct Groups {
    groups: Vec<DeadGroup>,
    index: HashMap<String, usize>,
}

impl Groups {
    fn add(&mut self, delivery: DeadDelivery) {
        let (outcome, error) = signature_parts(&delivery.attempts, delivery.last_error.as_deref());
        let signature = format!("{outcome}: {error}");
        let groups = &mut self.groups;
        let at = *self.index.entry(signature.clone()).or_insert_with(|| {
            groups.push(DeadGroup {
                signature,
                outcome,
                error,
                count: 0,
                deliveries: Vec::new(),
            });
            groups.len() - 1
        });
        self.groups[at].count += 1;
        self.groups[at].deliveries.push(delivery);
    }

    /// The most common signature first.
    fn sorted(mut self) -> Vec<DeadGroup> {
        self.groups.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| a.signature.cmp(&b.signature))
        });
        self.groups
    }
}

/// A dead letter's outcome and error line: its last attempt's, else (killed
/// before any attempt, or by the dead sync) `dead` and the delivery's own
/// last error.
pub(super) fn signature_parts(
    attempts: &[AttemptView],
    last_error: Option<&str>,
) -> (String, String) {
    let last = attempts.iter().max_by_key(|a| a.attempt);
    let outcome = last
        .and_then(|a| a.outcome.clone())
        .unwrap_or_else(|| "dead".to_string());
    let error = last
        .and_then(|a| a.error.as_deref())
        .or(last_error)
        .map(first_line)
        .filter(|line| !line.is_empty())
        .unwrap_or("(no error)")
        .to_string();
    (outcome, error)
}
