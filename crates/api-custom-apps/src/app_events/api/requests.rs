//! The routes' query strings and bodies, as `oxyc events` sends them, checked
//! and turned into what the operations take. Read only after the caller is
//! admitted, so a malformed request says nothing to a caller who may not ask.

use chrono::Utc;
use entity::apps;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::Deserialize;
use uuid::Uuid;

use super::destructive::{Discard, Purge};
use super::redeliver::{REDELIVERABLE, Selection};
use super::replay::{Replay, floor};
use super::reply::{ApiError, environment, listener, time};
use crate::app_events::subscriptions::listeners;

#[derive(Debug, Default, Deserialize)]
pub struct EnvironmentQuery {
    pub environment: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct FeedQuery {
    pub since: Option<String>,
    pub environment: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct FindQuery {
    #[serde(rename = "type")]
    pub event_type: Option<String>,
    pub key: Option<String>,
    pub environment: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RootQuery {
    pub root: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedeliverBody {
    pub to: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkRedeliverBody {
    pub to: Option<String>,
    pub status: Option<String>,
    pub since: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayBody {
    #[serde(rename = "type")]
    pub event_type: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub to: Option<String>,
    pub environment: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Default, Deserialize)]
pub struct DiscardBody {
    #[serde(rename = "type")]
    pub event_type: Option<String>,
    pub to: Option<String>,
    pub until: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PurgeBody {
    #[serde(rename = "type")]
    pub event_type: Option<String>,
    pub key: Option<String>,
    pub before: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct SwitchBody {
    pub enabled: Option<bool>,
}

fn required<'a>(field: &str, value: &'a Option<String>) -> Result<&'a str, ApiError> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| ApiError::BadRequest("missing_field", format!("`{field}` is required")))
}

pub(super) fn uuid(raw: &str, code: &'static str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(raw.trim())
        .map_err(|_| ApiError::NotFound(code, format!("`{raw}` names no event")))
}

pub(super) fn root(q: &RootQuery) -> Result<Uuid, ApiError> {
    let raw = required("root", &q.root)?;
    Uuid::parse_str(raw)
        .map_err(|_| ApiError::BadRequest("invalid_root", format!("`{raw}` is not an id")))
}

/// The event id `find` derives: `(type, key, environment)` in `org_id`.
pub(super) fn found_id(org_id: Uuid, q: &FindQuery) -> Result<Uuid, ApiError> {
    let event_type = required("type", &q.event_type)?;
    let key = required("key", &q.key)?;
    let environment = environment(q.environment.as_deref())?.name();
    Ok(oxy_custom_apps::events::event_id(
        org_id,
        &environment,
        event_type,
        key,
    ))
}

/// `<app>/<function>` of `org_id`: `Some((app id, function))`, or `None` for
/// an app the org does not have.
async fn resolve(
    db: &DatabaseConnection,
    org_id: Uuid,
    raw: &str,
) -> Result<Option<(Uuid, String)>, ApiError> {
    let (app, function) = listener(raw)?;
    let found = apps::Entity::find()
        .filter(apps::Column::OrgId.eq(org_id))
        .filter(apps::Column::Slug.eq(app))
        .one(db)
        .await
        .map_err(|e| ApiError::db("read the listener's app", e))?;
    Ok(found.map(|a| (a.id, function)))
}

/// One event's redelivery: `None` when `to` names an app the org lacks.
pub(super) async fn single(
    db: &DatabaseConnection,
    org_id: Uuid,
    event_id: Uuid,
    body: &RedeliverBody,
) -> Result<Option<Selection>, ApiError> {
    let listener = match body.to.as_deref() {
        Some(raw) => match resolve(db, org_id, raw).await? {
            Some(found) => Some(found),
            None => return Ok(None),
        },
        None => None,
    };
    Ok(Some(Selection {
        org_id,
        statuses: REDELIVERABLE.iter().map(|s| s.to_string()).collect(),
        event_id: Some(event_id),
        listener,
        since: None,
    }))
}

/// A bulk redelivery: `None` when `to` names an app the org lacks.
pub(super) async fn bulk(
    db: &DatabaseConnection,
    org_id: Uuid,
    body: &BulkRedeliverBody,
) -> Result<Option<Selection>, ApiError> {
    let to = required("to", &body.to)?;
    let status = required("status", &body.status)?;
    if !REDELIVERABLE.contains(&status) {
        return Err(ApiError::BadRequest(
            "invalid_status",
            format!("`status` is dead, expired or held, not `{status}`"),
        ));
    }
    let since = body
        .since
        .as_deref()
        .map(|raw| time("since", raw))
        .transpose()?;
    let Some(listener) = resolve(db, org_id, to).await? else {
        return Ok(None);
    };
    Ok(Some(Selection {
        org_id,
        statuses: vec![status.to_string()],
        event_id: None,
        listener: Some(listener),
        since,
    }))
}

/// A replay, `to` a function listening to the type in the environment now.
pub(super) async fn replay(
    db: &DatabaseConnection,
    org_id: Uuid,
    body: &ReplayBody,
) -> Result<Replay, ApiError> {
    let event_type = required("type", &body.event_type)?.to_string();
    let to = required("to", &body.to)?;
    let since = time("since", required("since", &body.since)?)?;
    let until = match body.until.as_deref() {
        Some(raw) => time("until", raw)?,
        None => Utc::now(),
    };
    let environment = environment(body.environment.as_deref())?.name();
    let (app, function) = listener(to)?;
    let found = listeners(db, org_id, &environment, &event_type)
        .await
        .map_err(|e| ApiError::db("read the type's listeners", e))?
        .into_iter()
        .find(|l| l.app_slug == app && l.function_name == function)
        .ok_or_else(|| {
            ApiError::Conflict(
                "not_a_listener",
                format!("`{to}` does not listen to `{event_type}` in {environment}"),
            )
        })?;
    Ok(Replay {
        org_id,
        event_type,
        environment,
        since: floor(since, Utc::now()),
        until,
        listener: (found.app_id, found.function_name),
        dry_run: body.dry_run,
    })
}

pub(super) async fn discard(
    db: &DatabaseConnection,
    org_id: Uuid,
    body: &DiscardBody,
) -> Result<Discard, ApiError> {
    let until = time("until", required("until", &body.until)?)?;
    let to = match body.to.as_deref() {
        Some(raw) => Some((raw.to_string(), resolve(db, org_id, raw).await?)),
        None => None,
    };
    Ok(Discard {
        org_id,
        event_type: body.event_type.clone().filter(|t| !t.trim().is_empty()),
        to,
        until,
    })
}

pub(super) fn purge(org_id: Uuid, body: &PurgeBody) -> Result<Purge, ApiError> {
    Ok(Purge {
        org_id,
        event_type: required("type", &body.event_type)?.to_string(),
        key: body.key.clone(),
        before: body
            .before
            .as_deref()
            .map(|raw| time("before", raw))
            .transpose()?,
    })
}

pub(super) fn enabled(body: &SwitchBody) -> Result<bool, ApiError> {
    body.enabled
        .ok_or_else(|| ApiError::BadRequest("missing_field", "`enabled` is required".into()))
}
