//! Registering a device for push, and removing it by token.

use axum::Json;
use axum::http::StatusCode;
use chrono::Utc;
use entity::device_tokens;
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DbErr, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Statement,
};
use serde::{Deserialize, Serialize};
use tracing::{info, instrument};
use uuid::Uuid;

use super::{MAX_APP_VERSION_LEN, MAX_DEVICE_NAME_LEN};
use crate::settings::store;
use crate::web_push_payload::{SubscriptionKeys, WebPushKeys};

#[derive(Debug, Deserialize)]
pub struct RegisterDevice {
    /// `apns` | `fcm` | `web`.
    pub platform: String,
    pub token: String,
    /// The label, as before `name` existed. `name` wins when both are sent.
    pub device_name: Option<String>,
    pub name: Option<String>,
    pub app_version: Option<String>,
    /// A `web` subscription's keys (`PushSubscription.toJSON().keys`). With
    /// them a push carries an encrypted payload naming its app and kind;
    /// without, it is a bodiless wake. Refused on any other platform.
    pub keys: Option<WebPushKeys>,
}

#[derive(Debug, Serialize)]
pub struct Registered {
    /// The device's id: what `PATCH`/`DELETE /api/notifications/devices/{id}`
    /// take.
    pub id: Uuid,
}

/// Longest token accepted. A Web Push endpoint URL is the longest of the three
/// by a wide margin and fits comfortably; APNs and FCM tokens are far shorter.
/// `token` is `TEXT`, so without this any authenticated caller can store
/// arbitrary bytes — and `notify` loads every one of a user's rows on every
/// single notification.
pub(super) const MAX_TOKEN_LEN: usize = 512;

/// Most devices one user may keep registered. Beyond this the oldest by
/// `last_seen_at` are dropped, which is the same thing an expiring token would
/// have done, just deliberately. Phones, tablets and a couple of browsers is
/// the real shape; anything past it is a client that never stops registering.
const MAX_DEVICES_PER_USER: u64 = 20;

/// What a registration says about the device.
#[derive(Debug, Clone, Copy)]
pub struct Registration<'a> {
    pub platform: &'a str,
    pub token: &'a str,
    pub name: Option<&'a str>,
    pub app_version: Option<&'a str>,
    /// A `web` subscription's keys, already checked.
    pub keys: Option<&'a SubscriptionKeys>,
}

/// `POST /api/notifications/devices` — register this device for push, and
/// answer with its id.
///
/// Collected before delivery exists on purpose: a token is only obtainable from
/// a device the user is holding, so gathering them has to work first, or the
/// day push ships it needs a user base that has already granted permission —
/// which it cannot have.
#[instrument(skip_all, fields(platform = %body.platform))]
pub async fn register_device(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Json(body): Json<RegisterDevice>,
) -> Result<Json<Registered>, StatusCode> {
    if !matches!(body.platform.as_str(), "apns" | "fcm" | "web") {
        return Err(StatusCode::BAD_REQUEST);
    }
    // Normalise ONCE, before anything matches on it: the stored value and the
    // value the conflict target is compared against have to be the same string,
    // or a token with stray whitespace writes a row that a re-registration
    // never finds.
    let token = body.token.trim().to_string();
    if token.is_empty() || token.len() > MAX_TOKEN_LEN {
        return Err(StatusCode::BAD_REQUEST);
    }
    // An iOS Simulator's stand-in token: a debug server delivers it through
    // `simctl`; a release server refuses to store it.
    if !super::super::push_routing::simulator_token_allowed(&body.platform, &token) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let name = body.name.as_deref().or(body.device_name.as_deref());
    if name.is_some_and(|n| n.len() > MAX_DEVICE_NAME_LEN)
        || body
            .app_version
            .as_deref()
            .is_some_and(|v| v.len() > MAX_APP_VERSION_LEN)
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let keys = match &body.keys {
        None => None,
        Some(keys) if body.platform == "web" => {
            Some(SubscriptionKeys::parse(keys).map_err(|_| StatusCode::BAD_REQUEST)?)
        }
        Some(_) => return Err(StatusCode::BAD_REQUEST),
    };
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let registration = Registration {
        platform: &body.platform,
        token: &token,
        name,
        app_version: body.app_version.as_deref(),
        keys: keys.as_ref(),
    };
    let id = upsert_device(&db, user.id, &registration, Utc::now().fixed_offset())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    prune_devices(&db, user.id).await;
    if let Err(e) = store::bump(&db, user.id).await {
        tracing::warn!(error = %e, "settings version not bumped after a registration");
    }
    info!(user = %user.id, "device registered for push");
    Ok(Json(Registered { id }))
}

/// Claim `(platform, token)` for `user_id`, as one statement, and return the
/// row's id.
///
/// A token identifies a DEVICE INSTALL, not a person. The same token can be
/// handed to a second account when a phone is passed on or an app is
/// reinstalled under a different login — so the row for this token must end
/// up owned by whoever registered last, or two users share it and one starts
/// receiving the other's notifications. When it changes hands, the previous
/// owner's settings for it (enabled, floor, OS state, name) go with them: the
/// row starts over as the new owner's.
///
/// One statement, not delete-then-insert. Two statements race: concurrent
/// registrations of the same token (a client retry, an app foregrounded
/// twice) both delete, then both insert, and the loser hits the
/// `UNIQUE (platform, token)` index and 500s on what is an idempotent
/// operation. Worse, the delete commits alone — so a failing insert leaves
/// the device deregistered and silently stops its push until the client
/// happens to try again.
///
/// A re-registration by the same owner bumps `last_seen_at`, keeps a name
/// the person gave the device (the registration's is only a default), and
/// takes the newer `app_version`.
///
/// A `web` subscription's keys are written when given and kept when not,
/// whoever registers: they belong to the subscription the endpoint names, not
/// to its owner, and never change for it — so a re-registration that does not
/// send them (an older tab) must not turn the device's pushes back into
/// bodiless wakes.
pub async fn upsert_device(
    db: &impl ConnectionTrait,
    user_id: Uuid,
    device: &Registration<'_>,
    now: chrono::DateTime<chrono::FixedOffset>,
) -> Result<Uuid, DbErr> {
    let (p256dh, auth) = device.keys.map(SubscriptionKeys::to_stored).unzip();
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO device_tokens \
               (id, user_id, platform, token, device_name, app_version, created_at, last_seen_at, \
                web_push_p256dh, web_push_auth) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $7, $8, $9) \
             ON CONFLICT (platform, token) DO UPDATE SET \
               web_push_p256dh = COALESCE(EXCLUDED.web_push_p256dh, device_tokens.web_push_p256dh), \
               web_push_auth = COALESCE(EXCLUDED.web_push_auth, device_tokens.web_push_auth), \
               device_name = CASE WHEN device_tokens.user_id = EXCLUDED.user_id \
                 THEN COALESCE(device_tokens.device_name, EXCLUDED.device_name) \
                 ELSE EXCLUDED.device_name END, \
               app_version = CASE WHEN device_tokens.user_id = EXCLUDED.user_id \
                 THEN COALESCE(EXCLUDED.app_version, device_tokens.app_version) \
                 ELSE EXCLUDED.app_version END, \
               enabled = CASE WHEN device_tokens.user_id = EXCLUDED.user_id \
                 THEN device_tokens.enabled ELSE true END, \
               min_level = CASE WHEN device_tokens.user_id = EXCLUDED.user_id \
                 THEN device_tokens.min_level END, \
               os_permission = CASE WHEN device_tokens.user_id = EXCLUDED.user_id \
                 THEN device_tokens.os_permission END, \
               os_disabled = CASE WHEN device_tokens.user_id = EXCLUDED.user_id \
                 THEN device_tokens.os_disabled END, \
               user_id = EXCLUDED.user_id, \
               last_seen_at = EXCLUDED.last_seen_at \
             RETURNING id",
            [
                Uuid::new_v4().into(),
                user_id.into(),
                device.platform.into(),
                device.token.into(),
                device.name.map(str::to_string).into(),
                device.app_version.map(str::to_string).into(),
                now.into(),
                p256dh.into(),
                auth.into(),
            ],
        ))
        .await?
        .ok_or_else(|| DbErr::RecordNotInserted)?;
    row.try_get("", "id")
}

/// Drop a user's least recently seen registrations past [`MAX_DEVICES_PER_USER`].
///
/// Best-effort: a failure here costs a stale row, never the registration that
/// just succeeded, so it must not turn a working call into a 500.
pub async fn prune_devices(db: &sea_orm::DatabaseConnection, user_id: Uuid) {
    let stale = device_tokens::Entity::find()
        .filter(device_tokens::Column::UserId.eq(user_id))
        .order_by_desc(device_tokens::Column::LastSeenAt)
        .offset(MAX_DEVICES_PER_USER)
        .all(db)
        .await;
    let Ok(stale) = stale else { return };
    if stale.is_empty() {
        return;
    }
    let ids: Vec<Uuid> = stale.iter().map(|r| r.id).collect();
    if let Err(e) = device_tokens::Entity::delete_many()
        .filter(device_tokens::Column::Id.is_in(ids.clone()))
        .exec(db)
        .await
    {
        tracing::warn!(user = %user_id, error = %e, "could not prune device tokens");
        return;
    }
    info!(user = %user_id, dropped = ids.len(), "pruned stale device tokens");
}

#[derive(Debug, Deserialize)]
pub struct UnregisterDevice {
    pub token: String,
}

/// `DELETE /api/notifications/devices` — stop pushing to this device.
///
/// What a phone calls on sign-out, so the next person to sign in on it is not
/// woken by the last one's work. Self-scoped: only the caller's own row for
/// the token is deleted, so a token cannot be used to deregister somebody
/// else's device. 204 whether or not a row existed — sign-out retries, and
/// "was it registered" is not something to answer about a token.
#[instrument(skip_all)]
pub async fn unregister_device(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Json(body): Json<UnregisterDevice>,
) -> Result<StatusCode, StatusCode> {
    // Normalised as registration normalises it, or a token registered with
    // stray whitespace could never be removed.
    let token = body.token.trim();
    if token.is_empty() || token.len() > MAX_TOKEN_LEN {
        return Err(StatusCode::BAD_REQUEST);
    }
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let removed = delete_device(user.id, token)
        .exec(&db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .rows_affected;
    if removed > 0
        && let Err(e) = store::bump(&db, user.id).await
    {
        tracing::warn!(error = %e, "settings version not bumped after an unregistration");
    }
    info!(user = %user.id, removed, "device unregistered from push");
    Ok(StatusCode::NO_CONTENT)
}

/// The delete [`unregister_device`] runs: `user_id`'s rows for `token`, on any
/// platform, and nobody else's.
pub fn delete_device(user_id: Uuid, token: &str) -> sea_orm::DeleteMany<device_tokens::Entity> {
    device_tokens::Entity::delete_many()
        .filter(device_tokens::Column::UserId.eq(user_id))
        .filter(device_tokens::Column::Token.eq(token))
}
