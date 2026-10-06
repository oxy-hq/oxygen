//! `GET /api/{workspace_id}/api-keys/{id}/activity` — what happened to one key
//! and what was done with it (API-tokens design §3.7, "where people see it").
//!
//! Browser session only, and only the key's owner (anyone else, and an unknown
//! id, read 404). Three bounded, indexed reads, all Postgres:
//!
//! - `events`: the key's lifecycle events and the actions performed with it,
//!   newest first, capped by `limit`. The audit log keeps a rolling window, so
//!   this is recent history, not all of it.
//! - `usage`: one row per day for the last 30 days, oldest first.
//! - `last_used`: from the newest usage row; before the first flush, from the
//!   token's own `last_used_at`.
//!
//! Counts appear within a minute: usage is accumulated in memory and flushed.

use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use entity::api_token_usage_daily as usage_daily;
use entity::prelude::ApiTokenUsageDaily;
use oxy::database::client::establish_connection;
use oxy_app_core::audit::{self, RequestActor, TOKEN_ID_KEY};
use oxy_auth::ApiKeyService;
use oxy_auth::extractor::{SessionAction, SessionOnly};
use oxy_shared::errors::OxyError;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::server::api::admin::audit::AuditEventDto;

const DEFAULT_LIMIT: u64 = 100;
const MAX_LIMIT: u64 = 500;
/// Days of usage returned, today included.
const USAGE_DAYS: i64 = 30;
/// The only metadata an event exposes here (contract amendment, #3406).
const EVENT_METADATA_KEYS: &[&str] = &[TOKEN_ID_KEY, "old_expires_at", "new_expires_at"];

pub struct ViewActivity;
impl SessionAction for ViewActivity {
    const REFUSAL: &'static str = "activity requires a browser session";
}

#[derive(Deserialize)]
pub struct ActivityQuery {
    pub limit: Option<u64>,
}

/// The admin audit row shape, plus the three metadata keys the UI reads.
#[derive(Serialize)]
pub struct ActivityEvent {
    #[serde(flatten)]
    pub event: AuditEventDto,
    pub metadata: Value,
}

#[derive(Serialize)]
pub struct UsageDay {
    pub day: NaiveDate,
    pub requests: i64,
    pub errors_4xx: i64,
    pub errors_5xx: i64,
}

#[derive(Serialize)]
pub struct LastUsed {
    pub at: DateTime<Utc>,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    /// The matched route template, never the raw path.
    pub route: Option<String>,
}

#[derive(Serialize)]
pub struct ActivityResponse {
    pub events: Vec<ActivityEvent>,
    pub usage: Vec<UsageDay>,
    pub last_used: Option<LastUsed>,
}

fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

/// Activity for one of the caller's API keys
pub async fn get_api_key_activity(
    _: SessionOnly<ViewActivity>,
    actor: RequestActor,
    Path((_workspace_id, id)): Path<(Uuid, String)>,
    Query(query): Query<ActivityQuery>,
) -> Response {
    let Ok(key_id) = Uuid::parse_str(&id) else {
        return error(StatusCode::NOT_FOUND, "API key not found");
    };
    match load(key_id, actor.id, page_size(query.limit)).await {
        Ok(Some(activity)) => Json(activity).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "API key not found"),
        Err(e) => {
            tracing::error!("Failed to load API key activity: {e}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
        }
    }
}

/// `None` when the key is not the caller's (or does not exist).
async fn load(
    key_id: Uuid,
    user_id: Uuid,
    limit: u64,
) -> Result<Option<ActivityResponse>, OxyError> {
    let db = establish_connection().await?;
    let Some(key) = ApiKeyService::find_owned_key(&db, key_id, user_id).await? else {
        return Ok(None);
    };
    // A key's token id is its own id (see the `api_tokens` migration).
    let fallback = fallback_last_used(&db, &key).await?;
    token_activity(&db, key_id, fallback, limit).await.map(Some)
}

/// The page size a caller asked for, bounded.
pub(crate) fn page_size(limit: Option<u64>) -> u64 {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

/// One token's Activity: its lifecycle events and the actions performed with
/// it, 30 days of usage, and where it was last seen. `fallback` is the
/// last-used instant to report when no usage row exists yet.
///
/// A lifecycle event is written once per org the token reaches (design §3.7),
/// so the same event arrives as several rows; they share a `metadata.event_id`
/// and are shown once.
pub(crate) async fn token_activity(
    db: &DatabaseConnection,
    token_id: Uuid,
    fallback: Option<LastUsed>,
    limit: u64,
) -> Result<ActivityResponse, OxyError> {
    let rows = audit::events_for_token(db, token_id, limit)
        .await
        .map_err(db_err)?;
    let events = distinct_events(rows);
    let rows = usage_window(db, token_id).await?;
    let last_used = match newest_usage(db, token_id).await? {
        Some(row) => Some(last_used_from(row)),
        None => fallback,
    };
    Ok(ActivityResponse {
        events,
        usage: rows.into_iter().map(usage_day).collect(),
        last_used,
    })
}

/// One row per event: the copies written to each org share an `event_id`.
fn distinct_events(rows: Vec<entity::audit_events::Model>) -> Vec<ActivityEvent> {
    let mut seen = std::collections::HashSet::new();
    rows.into_iter()
        .filter(
            |row| match row.metadata.get("event_id").and_then(Value::as_str) {
                Some(event_id) => seen.insert(event_id.to_string()),
                None => true,
            },
        )
        .map(activity_event)
        .collect()
}

/// A token's Activity **as one org sees it**: only the events in that org's
/// chain. For a token that is not the org's own — a person's, which may act
/// in other orgs too — there is no usage history and no last IP, user agent or
/// route: those describe requests the org has no claim to see. `fallback` is
/// then all `last_used` says.
pub(crate) async fn token_activity_in_org(
    db: &DatabaseConnection,
    token_id: Uuid,
    org_id: Uuid,
    fallback: Option<LastUsed>,
    limit: u64,
) -> Result<ActivityResponse, OxyError> {
    let rows = audit::events_for_token_in_org(db, token_id, org_id, limit)
        .await
        .map_err(db_err)?;
    Ok(ActivityResponse {
        events: distinct_events(rows),
        usage: Vec::new(),
        last_used: fallback,
    })
}

/// A last-used instant with nothing else known about the request.
pub(crate) fn last_used_at(at: DateTime<Utc>) -> LastUsed {
    LastUsed {
        at,
        ip: None,
        user_agent: None,
        route: None,
    }
}

fn db_err(e: sea_orm::DbErr) -> OxyError {
    OxyError::DBError(format!("api key activity: {e}"))
}

fn activity_event(row: entity::audit_events::Model) -> ActivityEvent {
    let metadata = event_metadata(&row.metadata);
    ActivityEvent {
        event: row.into(),
        metadata,
    }
}

/// Only the keys the contract names, and only when the row has them.
fn event_metadata(stored: &Value) -> Value {
    let mut out = Map::new();
    for key in EVENT_METADATA_KEYS {
        if let Some(value) = stored.get(*key) {
            out.insert((*key).to_string(), value.clone());
        }
    }
    Value::Object(out)
}

/// The last 30 days, oldest first. At most 30 rows by the primary key.
async fn usage_window(
    db: &DatabaseConnection,
    token_id: Uuid,
) -> Result<Vec<usage_daily::Model>, OxyError> {
    let since = (Utc::now() - Duration::days(USAGE_DAYS - 1)).date_naive();
    ApiTokenUsageDaily::find()
        .filter(usage_daily::Column::TokenId.eq(token_id))
        .filter(usage_daily::Column::Day.gte(since))
        .order_by_asc(usage_daily::Column::Day)
        .limit(USAGE_DAYS as u64 + 1)
        .all(db)
        .await
        .map_err(db_err)
}

async fn newest_usage(
    db: &DatabaseConnection,
    token_id: Uuid,
) -> Result<Option<usage_daily::Model>, OxyError> {
    ApiTokenUsageDaily::find()
        .filter(usage_daily::Column::TokenId.eq(token_id))
        .order_by_desc(usage_daily::Column::Day)
        .limit(1)
        .one(db)
        .await
        .map_err(db_err)
}

fn usage_day(row: usage_daily::Model) -> UsageDay {
    UsageDay {
        day: row.day,
        requests: row.requests,
        errors_4xx: row.errors_4xx,
        errors_5xx: row.errors_5xx,
    }
}

fn last_used_from(row: usage_daily::Model) -> LastUsed {
    LastUsed {
        at: row.last_seen_at.into(),
        ip: row.last_ip,
        user_agent: row.last_user_agent,
        route: row.last_route,
    }
}

/// Before any usage row exists: the token's own `last_used_at` (or the key's,
/// when it has no token row yet). When, but not from where.
async fn fallback_last_used(
    db: &DatabaseConnection,
    key: &entity::api_keys::Model,
) -> Result<Option<LastUsed>, OxyError> {
    let token_at = ApiKeyService::mirror_of(db, key.id)
        .await?
        .and_then(|t| t.last_used_at);
    Ok(token_at
        .or(key.last_used_at)
        .map(|at| last_used_at(at.into())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_metadata_exposes_only_the_contract_keys() {
        let stored = json!({
            "token_id": "t",
            "old_expires_at": "2026-10-01T00:00:00+00:00",
            "new_expires_at": null,
            "token_name": "deploy",
            "display_prefix": "oxy_pat_Ab3x",
            "api_key_id": "k",
        });
        assert_eq!(
            event_metadata(&stored),
            json!({
                "token_id": "t",
                "old_expires_at": "2026-10-01T00:00:00+00:00",
                "new_expires_at": null,
            })
        );
    }

    #[test]
    fn an_event_without_them_carries_an_empty_object() {
        assert_eq!(event_metadata(&json!({ "surface": "admin" })), json!({}));
        assert_eq!(event_metadata(&Value::Null), json!({}));
    }

    #[test]
    fn the_limit_is_clamped_at_both_ends() {
        for (asked, got) in [(None, 100), (Some(0), 1), (Some(7), 7), (Some(9999), 500)] {
            assert_eq!(asked.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT), got);
        }
    }
}
