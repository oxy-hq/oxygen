//! A person's push devices: registration ([`register`]), and each device's
//! own settings and OS state.
//!
//! - `GET /api/notifications/devices` — mine, most recently seen first.
//! - `PATCH /api/notifications/devices/{id}` — `{ name?, enabled?,
//!   min_level?, os_permission?, os_disabled?, app_version? }`. A phone calls
//!   it on launch to report its OS state; every call bumps `last_seen_at`.
//!   Answers with the device.
//! - `DELETE /api/notifications/devices/{id}` — `204`.
//!
//! Self-scoped: a device that is not the caller's is `404`, never `403`.
//! Platforms are named as a person knows them: `apns` is `ios`, `fcm` is
//! `android`.

pub mod register;

use axum::Json;
use axum::body::Bytes;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{SecondsFormat, Utc};
use entity::device_tokens;
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::instrument;
use uuid::Uuid;

use super::catalog::model::Level;
use super::settings::model::{DeviceSettings, OsPermission};
use super::settings::patch::present;
use super::settings::store;

/// Longest device label accepted: a user-supplied display string ("Kitchen
/// iPad") on a `TEXT` column, written by any authenticated caller and read
/// back on every notification.
pub const MAX_DEVICE_NAME_LEN: usize = 120;
/// Longest `app_version` accepted.
pub const MAX_APP_VERSION_LEN: usize = 64;
/// Most `os_disabled` ids one device reports, and the longest one.
const MAX_OS_DISABLED: usize = 256;
const MAX_OS_DISABLED_LEN: usize = 200;

/// One device, as `GET /api/notifications/devices` and the settings
/// document list it. Every field present, `null` when unset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceView {
    pub id: Uuid,
    /// `web` | `ios` | `android`.
    pub platform: &'static str,
    pub name: Option<String>,
    pub app_version: Option<String>,
    pub created_at: String,
    pub last_seen_at: String,
    pub enabled: bool,
    pub min_level: Option<Level>,
    pub os_permission: Option<OsPermission>,
    pub os_disabled: Vec<String>,
}

/// The platform as a person knows it.
pub fn platform_name(stored: &str) -> &'static str {
    match stored {
        "apns" => "ios",
        "fcm" => "android",
        _ => "web",
    }
}

impl DeviceView {
    pub fn of(row: &device_tokens::Model) -> Self {
        let settings = DeviceSettings::of(row);
        let at = |t: &chrono::DateTime<chrono::FixedOffset>| {
            t.with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Millis, true)
        };
        Self {
            id: row.id,
            platform: platform_name(&row.platform),
            name: row.device_name.clone(),
            app_version: row.app_version.clone(),
            created_at: at(&row.created_at),
            last_seen_at: at(&row.last_seen_at),
            enabled: settings.enabled,
            min_level: settings.min_level,
            os_permission: settings.os_permission,
            os_disabled: settings.os_disabled,
        }
    }
}

/// `user_id`'s devices, most recently seen first.
pub async fn list_for(db: &DatabaseConnection, user_id: Uuid) -> Result<Vec<DeviceView>, DbErr> {
    Ok(device_tokens::Entity::find()
        .filter(device_tokens::Column::UserId.eq(user_id))
        .order_by_desc(device_tokens::Column::LastSeenAt)
        .order_by_asc(device_tokens::Column::Id)
        .all(db)
        .await?
        .iter()
        .map(DeviceView::of)
        .collect())
}

/// `PATCH /api/notifications/devices/{id}`'s body. Absent leaves a value;
/// `null` clears a nullable one.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevicePatch {
    #[serde(default, deserialize_with = "present")]
    pub name: Option<Option<String>>,
    pub enabled: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    pub min_level: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    pub os_permission: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    pub os_disabled: Option<Option<Vec<String>>>,
    #[serde(default, deserialize_with = "present")]
    pub app_version: Option<Option<String>>,
}

/// Why a device change was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceError {
    /// Not the caller's, or no such device.
    NotFound,
    /// `400 { error }`.
    BadRequest(&'static str),
    Internal,
}

impl From<DbErr> for DeviceError {
    fn from(_: DbErr) -> Self {
        DeviceError::Internal
    }
}

/// A trimmed optional text, `None` when empty; `Err` past `max` bytes.
fn text(
    value: Option<String>,
    max: usize,
    code: &'static str,
) -> Result<Option<String>, DeviceError> {
    let value = value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    match value {
        Some(v) if v.len() > max => Err(DeviceError::BadRequest(code)),
        v => Ok(v),
    }
}

/// `row` with `patch` applied, checked. Pure, apart from the clock.
pub fn merge(
    row: device_tokens::Model,
    patch: DevicePatch,
) -> Result<device_tokens::ActiveModel, DeviceError> {
    let bad = DeviceError::BadRequest;
    let mut next: device_tokens::ActiveModel = row.into();
    if let Some(name) = patch.name {
        next.device_name = Set(text(name, MAX_DEVICE_NAME_LEN, "bad_name")?);
    }
    if let Some(version) = patch.app_version {
        next.app_version = Set(text(version, MAX_APP_VERSION_LEN, "bad_app_version")?);
    }
    if let Some(enabled) = patch.enabled {
        next.enabled = Set(enabled);
    }
    if let Some(level) = patch.min_level {
        let level = level
            .map(|l| Level::parse(&l).ok_or(bad("bad_level")))
            .transpose()?;
        next.min_level = Set(level.map(|l| l.as_str().to_string()));
    }
    if let Some(permission) = patch.os_permission {
        let permission = permission
            .map(|p| OsPermission::parse(&p).ok_or(bad("bad_os_permission")))
            .transpose()?;
        next.os_permission = Set(permission.map(|p| p.as_str().to_string()));
    }
    if let Some(ids) = patch.os_disabled {
        let mut ids = ids.unwrap_or_default();
        ids.sort();
        ids.dedup();
        let valid = ids.len() <= MAX_OS_DISABLED
            && ids
                .iter()
                .all(|id| !id.trim().is_empty() && id.len() <= MAX_OS_DISABLED_LEN);
        if !valid {
            return Err(bad("bad_os_disabled"));
        }
        next.os_disabled = Set((!ids.is_empty()).then(|| json!(ids)));
    }
    next.last_seen_at = Set(Utc::now().fixed_offset());
    Ok(next)
}

/// `user_id`'s device `id`, or [`DeviceError::NotFound`].
async fn mine(
    db: &DatabaseConnection,
    user_id: Uuid,
    id: Uuid,
) -> Result<device_tokens::Model, DeviceError> {
    device_tokens::Entity::find_by_id(id)
        .one(db)
        .await?
        .filter(|d| d.user_id == user_id)
        .ok_or(DeviceError::NotFound)
}

/// Apply `patch` to `user_id`'s device `id`.
pub async fn update(
    db: &DatabaseConnection,
    user_id: Uuid,
    id: Uuid,
    patch: DevicePatch,
) -> Result<DeviceView, DeviceError> {
    let row = mine(db, user_id, id).await?;
    let updated = merge(row, patch)?.update(db).await?;
    store::bump(db, user_id).await?;
    Ok(DeviceView::of(&updated))
}

/// Remove `user_id`'s device `id`.
pub async fn remove(db: &DatabaseConnection, user_id: Uuid, id: Uuid) -> Result<(), DeviceError> {
    let row = mine(db, user_id, id).await?;
    device_tokens::Entity::delete_by_id(row.id).exec(db).await?;
    store::bump(db, user_id).await?;
    Ok(())
}

fn respond(e: DeviceError) -> Response {
    match e {
        DeviceError::NotFound => StatusCode::NOT_FOUND.into_response(),
        DeviceError::BadRequest(code) => {
            (StatusCode::BAD_REQUEST, Json(json!({ "error": code }))).into_response()
        }
        DeviceError::Internal => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// `GET /api/notifications/devices`
#[instrument(skip_all, fields(user = %user.id))]
pub async fn list(AuthenticatedUserExtractor(user): AuthenticatedUserExtractor) -> Response {
    let Ok(db) = establish_connection().await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    match list_for(&db, user.id).await {
        Ok(devices) => Json(devices).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// `PATCH /api/notifications/devices/{id}`
#[instrument(skip_all, fields(user = %user.id, device = %id))]
pub async fn patch(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(id): Path<Uuid>,
    body: Bytes,
) -> Response {
    let Ok(patch) = serde_json::from_slice::<DevicePatch>(&body) else {
        return respond(DeviceError::BadRequest("invalid_body"));
    };
    let Ok(db) = establish_connection().await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    match update(&db, user.id, id, patch).await {
        Ok(device) => Json(device).into_response(),
        Err(e) => respond(e),
    }
}

/// `DELETE /api/notifications/devices/{id}`
#[instrument(skip_all, fields(user = %user.id, device = %id))]
pub async fn delete(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(id): Path<Uuid>,
) -> Response {
    let Ok(db) = establish_connection().await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    match remove(&db, user.id, id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => respond(e),
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
