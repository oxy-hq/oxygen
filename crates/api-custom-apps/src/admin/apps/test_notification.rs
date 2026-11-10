//! The console's "Test notifications": send yourself one notification as an
//! app, to see it reach your phone.
//!
//! - `GET /api/customer-apps/{id}/notification-kinds` — the kinds the app's live
//!   build declares, each with its label, level and whether a mute applies, and
//!   whether the app can tell the caller anything at all. The catalogue is the
//!   notifications surface's, read through the [`LiveKinds`] port `oxy-server`
//!   layers onto these routes (one surface never imports another).
//! - `POST /api/customer-apps/{id}/notification-test` `{ "kind": "…" }` — one
//!   notification of that kind, from the app, to the caller alone.
//!
//! The send is `ctx.notify`'s ([`send`]): the content in the org's store,
//! an inbox row, then a queued delivery that applies the caller's mutes. So a
//! muted kind does not reach their devices, a critical one does, and tapping
//! it opens the app. Push and the inbox only, never email or Slack, as with
//! the platform's own test notice (`POST /api/notifications/test`).
//!
//! Only to the caller, and only when the app could tell them anyway
//! ([`audience`]): the console is staff's, and this must not become a way to
//! send as an app to someone it never reaches, nor to staff outside its org.
//!
//! One test per person per app every [`COOLDOWN`], counted from the tests' own
//! sends (an idempotency key per ten-second window), not from the app's real
//! notifications. Two clicks in one window are one send: the key's unique
//! index decides, so replicas need no lock.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::Path;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use entity::{apps, notification_sends};
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::instrument;
use uuid::Uuid;

use oxy_app_core::agent_refusal::RefuseSandboxAgent;
use oxy_app_core::audience::PersonReach;
use oxy_app_core::notifications::catalog::live_kinds::{KindSummary, LiveKinds};
use oxy_app_core::notifications::platform::self_test::{COOLDOWN, cooldown_left};
use oxy_app_core::notifications::send::{AppRef, Outgoing, SendError, key_hash, send};

/// The environment a test's send is recorded under: the live build's, whose
/// `ctx.notify` calls the kinds describe. It scopes the idempotency key only.
pub const ENVIRONMENT: &str = "production";

/// One kind, as the console lists it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct KindView {
    pub kind: String,
    pub label: String,
    pub description: Option<String>,
    /// `passive`, `active`, `time_sensitive` or `critical`.
    pub level: &'static str,
    /// The labels from the top category down to its own.
    pub category: Vec<String>,
    /// Whether a person's mute stops it: false for a critical kind.
    pub mutable: bool,
}

/// Every kind the catalogue describes or a function may send, in kind order.
pub fn kinds_of(kinds: &[KindSummary]) -> Vec<KindView> {
    kinds
        .iter()
        .map(|info| KindView {
            level: info.level.as_str(),
            kind: info.kind.clone(),
            label: info.label.clone(),
            description: info.description.clone(),
            category: info.category_labels.clone(),
            mutable: info.mutable,
        })
        .collect()
}

/// What a test send did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestOutcome {
    Sent {
        id: Uuid,
    },
    /// The kind is not one the app's live build declares.
    UnknownKind,
    /// The app cannot tell the caller anything: not in its audience.
    NotInAudience,
    RateLimited {
        retry_after_secs: u64,
    },
}

/// Send `user_id` one notification of `kind` from `app`, at `now`, reading the
/// app's kinds through `catalogue`.
pub async fn send_test(
    db: &DatabaseConnection,
    catalogue: &dyn LiveKinds,
    user_id: Uuid,
    app: &apps::Model,
    kind: &str,
    now: DateTime<Utc>,
) -> Result<TestOutcome, SendError> {
    let kinds = catalogue.live_kinds(db, app).await?;
    let Some(info) = kinds.into_iter().find(|k| k.kind == kind) else {
        return Ok(TestOutcome::UnknownKind);
    };
    if !in_audience(db, user_id, app).await? {
        return Ok(TestOutcome::NotInAudience);
    }
    if let Some(retry_after_secs) = cooling_down(db, user_id, app.id, now).await? {
        return Ok(TestOutcome::RateLimited { retry_after_secs });
    }
    let out = Outgoing {
        org_id: app.org_id,
        app: Some(AppRef {
            id: app.id,
            slug: app.slug.clone(),
            name: app.name.clone(),
        }),
        environment: ENVIRONMENT.to_string(),
        // Queued like `ctx.notify`'s, so the test exercises the same delivery.
        workspace_id: Some(app.project_id),
        kind: kind.to_string(),
        critical: info.critical(),
        title: title(&info.label),
        body: Some(body(now)),
        link: None,
        email: None,
        slack: false,
        idempotency_key: Some(window_key(user_id, window(now))),
        thread_key: None,
        recipients: vec![user_id],
    };
    // `oxy_app_core`'s send, not the notifications surface's inline-delivering
    // wrapper: this send always has a workspace, so it is always queued.
    let sent = send(db, out).await?;
    if sent.deduplicated {
        // Another click in this window sent it first.
        let retry_after_secs = cooling_down(db, user_id, app.id, now).await?.unwrap_or(1);
        return Ok(TestOutcome::RateLimited { retry_after_secs });
    }
    Ok(TestOutcome::Sent { id: sent.id })
}

/// Whether `app` may tell `user_id` anything: the rule `ctx.notify`'s
/// recipients pass.
pub async fn in_audience(
    db: &DatabaseConnection,
    user_id: Uuid,
    app: &apps::Model,
) -> Result<bool, DbErr> {
    let reach = PersonReach::load(db, user_id).await?;
    Ok(reach.standing(app.org_id, app.id).admits())
}

/// The ten-second window `at` falls in.
pub fn window(at: DateTime<Utc>) -> i64 {
    let width = i64::try_from(COOLDOWN.as_secs()).unwrap_or(10).max(1);
    at.timestamp().div_euclid(width)
}

/// A test's idempotency key for `user_id` in `window`. Scoped to one app and
/// environment by the send itself.
pub fn window_key(user_id: Uuid, window: i64) -> String {
    format!("console-test|{user_id}|{window}")
}

/// Seconds left before `user_id` may test `app_id` again, if a test of theirs
/// was sent in this window or the one before it.
async fn cooling_down(
    db: &DatabaseConnection,
    user_id: Uuid,
    app_id: Uuid,
    now: DateTime<Utc>,
) -> Result<Option<u64>, DbErr> {
    let current = window(now);
    let keys = [current, current - 1].map(|w| key_hash(&window_key(user_id, w)));
    let latest = notification_sends::Entity::find()
        .filter(notification_sends::Column::AppId.eq(app_id))
        .filter(notification_sends::Column::Environment.eq(ENVIRONMENT))
        .filter(notification_sends::Column::IdempotencyKeySha256.is_in(keys))
        .order_by_desc(notification_sends::Column::CreatedAt)
        .one(db)
        .await?;
    Ok(latest.and_then(|row| cooldown_left(row.created_at.with_timezone(&Utc), now)))
}

pub fn title(label: &str) -> String {
    format!("Test: {label}")
}

pub fn body(at: DateTime<Utc>) -> String {
    format!(
        "Sent from the admin console at {} UTC on {}.",
        at.format("%H:%M:%S"),
        at.format("%B %-d")
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TestRequest {
    kind: String,
}

/// The kind a request names. Read from raw bytes, so a malformed body or one
/// with a field this route does not take is a 400 rather than ignored.
pub fn parse(raw: &[u8]) -> Result<String, &'static str> {
    let req: TestRequest = serde_json::from_slice(raw).map_err(|_| "invalid_body")?;
    let kind = req.kind.trim();
    if kind.is_empty() {
        return Err("invalid_body");
    }
    Ok(kind.to_string())
}

/// `GET /api/customer-apps/{id}/notification-kinds`
#[instrument(skip_all, fields(app = %id, user = %user.id))]
pub async fn list_kinds(
    Extension(catalogue): Extension<Arc<dyn LiveKinds>>,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(id): Path<Uuid>,
) -> Response {
    let Ok(db) = establish_connection().await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let app = match apps::Entity::find_by_id(id).one(&db).await {
        Ok(Some(app)) => app,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal(e.into()),
    };
    let kinds = match catalogue.live_kinds(&db, &app).await {
        Ok(kinds) => kinds,
        Err(e) => return internal(e.into()),
    };
    let reachable = match in_audience(&db, user.id, &app).await {
        Ok(r) => r,
        Err(e) => return internal(e.into()),
    };
    Json(json!({ "reachable": reachable, "kinds": kinds_of(&kinds) })).into_response()
}

/// `POST /api/customer-apps/{id}/notification-test`
#[instrument(skip_all, fields(app = %id, user = %user.id))]
pub async fn send_test_notification(
    _: RefuseSandboxAgent,
    Extension(catalogue): Extension<Arc<dyn LiveKinds>>,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(id): Path<Uuid>,
    raw: Bytes,
) -> Response {
    let kind = match parse(&raw) {
        Ok(kind) => kind,
        Err(code) => return refuse(StatusCode::BAD_REQUEST, code),
    };
    let Ok(db) = establish_connection().await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let app = match apps::Entity::find_by_id(id).one(&db).await {
        Ok(Some(app)) => app,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal(e.into()),
    };
    match send_test(&db, catalogue.as_ref(), user.id, &app, &kind, Utc::now()).await {
        Ok(TestOutcome::Sent { id }) => Json(json!({ "id": id, "kind": kind })).into_response(),
        Ok(TestOutcome::UnknownKind) => refuse(StatusCode::BAD_REQUEST, "unknown_kind"),
        Ok(TestOutcome::NotInAudience) => refuse(StatusCode::CONFLICT, "not_in_audience"),
        Ok(TestOutcome::RateLimited { retry_after_secs }) => rate_limited(retry_after_secs),
        Err(SendError::NoStore(_)) => refuse(StatusCode::CONFLICT, "no_store"),
        Err(e) => internal(e),
    }
}

/// A refusal with a code the console can branch on.
fn refuse(status: StatusCode, error: &str) -> Response {
    (status, Json(json!({ "error": error }))).into_response()
}

fn rate_limited(retry_after: u64) -> Response {
    let body = json!({ "error": "rate_limited", "retry_after": retry_after });
    let mut response = (StatusCode::TOO_MANY_REQUESTS, Json(body)).into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from(retry_after));
    response
}

fn internal(e: SendError) -> Response {
    tracing::warn!(error = %e, "app test notification failed");
    match e {
        SendError::Store(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[cfg(test)]
#[path = "test_notification_tests.rs"]
mod tests;
