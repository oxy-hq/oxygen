use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use chrono::Utc;
use entity::{device_tokens, notifications};
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};
use serde::{Deserialize, Serialize};
use tracing::{info, instrument};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct InboxQuery {
    /// Unread only. The default, because that is what a badge and a dropdown
    /// both want, and the read tail is unbounded.
    #[serde(default = "yes")]
    pub unread_only: bool,
    pub limit: Option<u64>,
    /// Narrow to one org. Absent means every org the user has notifications
    /// from, which is what a single global badge wants; the org surfaces pass
    /// it so their own badge counts only their own work.
    pub org_id: Option<Uuid>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ReadAllQuery {
    /// Same meaning as on the inbox, and the reason it is stated at all:
    /// `read-all` is a WRITE, so "all" has to be a deliberate scope. Clearing
    /// another org's unread as a side effect of clearing this one's is not
    /// something a user can undo.
    ///
    /// A QUERY parameter, not a body. `Option<Json<T>>` resolves to `None` when
    /// extraction fails for ANY reason — a wrong `Content-Type`, malformed
    /// JSON — so a caller who meant to scope this write to one org and got the
    /// header wrong would silently clear every org instead. There is no such
    /// failure mode here: a malformed query is a 400, and it matches `inbox`,
    /// which already takes `org_id` this way.
    pub org_id: Option<Uuid>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Serialize)]
pub struct InboxResponse {
    pub unread: u64,
    pub items: Vec<notifications::Model>,
}

#[derive(Debug, Deserialize)]
pub struct RegisterDevice {
    /// `apns` | `fcm` | `web`.
    pub platform: String,
    pub token: String,
    pub device_name: Option<String>,
}

const MAX_PAGE: u64 = 100;

/// Longest token accepted. A Web Push endpoint URL is the longest of the three
/// by a wide margin and fits comfortably; APNs and FCM tokens are far shorter.
/// `token` is `TEXT`, so without this any authenticated caller can store
/// arbitrary bytes — and `notify` loads every one of a user's rows on every
/// single notification.
const MAX_TOKEN_LEN: usize = 512;

/// Longest device label accepted. A user-supplied display string ("Kitchen
/// iPad") on a `TEXT` column, bounded for the same reason the token is: it is
/// written by any authenticated caller and read back on every notification.
const MAX_DEVICE_NAME_LEN: usize = 120;

/// Most devices one user may keep registered. Beyond this the oldest by
/// `last_seen_at` are dropped, which is the same thing an expiring token would
/// have done, just deliberately. Phones, tablets and a couple of browsers is
/// the real shape; anything past it is a client that never stops registering.
const MAX_DEVICES_PER_USER: u64 = 20;

/// `GET /api/notifications` — my inbox.
///
/// Self-scoped: the filter is `user_id = me`, which is the authorization. There
/// is no org gate, deliberately — a frontline worker holds no org membership by
/// design and is precisely who overdue-work notifications are for.
#[instrument(skip_all)]
pub async fn inbox(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Query(q): Query<InboxQuery>,
) -> Result<Json<InboxResponse>, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;

    // An org that ended the request's token's reach (API-tokens design §5) is
    // left out of the count and the page alike — including when `org_id` asks
    // for it by name. Nothing is excluded for a session or a legacy key.
    let blocked = user.blocked_orgs();
    let scope = |f: sea_orm::Select<notifications::Entity>| {
        let f = match q.org_id {
            Some(org) => f.filter(notifications::Column::OrgId.eq(org)),
            None => f,
        };
        if blocked.is_empty() {
            f
        } else {
            f.filter(notifications::Column::OrgId.is_not_in(blocked.iter().copied()))
        }
    };

    let unread = scope(notifications::Entity::find())
        .filter(notifications::Column::UserId.eq(user.id))
        .filter(notifications::Column::ReadAt.is_null())
        .count(&db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut find = scope(notifications::Entity::find())
        .filter(notifications::Column::UserId.eq(user.id))
        .order_by_desc(notifications::Column::CreatedAt)
        .limit(q.limit.unwrap_or(50).clamp(1, MAX_PAGE));
    if q.unread_only {
        find = find.filter(notifications::Column::ReadAt.is_null());
    }

    let items = find
        .all(&db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(InboxResponse { unread, items }))
}

/// `POST /api/notifications/{id}/read`
#[instrument(skip_all, fields(id = %id))]
pub async fn mark_read(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;

    let Some(row) = notifications::Entity::find_by_id(id)
        .one(&db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    else {
        return Err(StatusCode::NOT_FOUND);
    };
    // 404 rather than 403: somebody else's notification must not be confirmed
    // to exist by the shape of the refusal. Nor one of an org the request's
    // token does not reach, which the inbox does not list either.
    if row.user_id != user.id || !user.reaches_org(row.org_id) {
        return Err(StatusCode::NOT_FOUND);
    }
    // Already read is a no-op rather than an error — a double tap on a phone is
    // the normal case, not a mistake worth surfacing.
    if row.read_at.is_some() {
        return Ok(StatusCode::NO_CONTENT);
    }

    let mut update: notifications::ActiveModel = row.into();
    update.read_at = Set(Some(Utc::now().fixed_offset()));
    update
        .update(&db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/notifications/read-all`
#[instrument(skip_all)]
pub async fn mark_all_read(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Query(q): Query<ReadAllQuery>,
) -> Result<StatusCode, StatusCode> {
    let scope = q.org_id;
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let mut update = notifications::Entity::update_many()
        .col_expr(
            notifications::Column::ReadAt,
            sea_orm::sea_query::Expr::value(Utc::now().fixed_offset()),
        )
        .filter(notifications::Column::UserId.eq(user.id))
        .filter(notifications::Column::ReadAt.is_null());
    if let Some(org) = scope {
        update = update.filter(notifications::Column::OrgId.eq(org));
    }
    // "All" is all the request's token reaches: an org that blocked it keeps
    // its unread, as the inbox keeps it out of the count.
    let blocked = user.blocked_orgs();
    if !blocked.is_empty() {
        update = update.filter(notifications::Column::OrgId.is_not_in(blocked.iter().copied()));
    }
    update
        .exec(&db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/notifications/devices` — register this device for push.
///
/// Collected before delivery exists on purpose: a token is only obtainable from
/// a device the user is holding, so gathering them has to work first, or the
/// day push ships it needs a user base that has already granted permission —
/// which it cannot have.
#[instrument(skip_all, fields(platform = %body.platform))]
pub async fn register_device(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Json(body): Json<RegisterDevice>,
) -> Result<StatusCode, StatusCode> {
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
    if body
        .device_name
        .as_deref()
        .is_some_and(|n| n.len() > MAX_DEVICE_NAME_LEN)
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;

    let now = Utc::now().fixed_offset();

    // A token identifies a DEVICE INSTALL, not a person. The same token can be
    // handed to a second account when a phone is passed on or an app is
    // reinstalled under a different login — so the row for this token must end
    // up owned by whoever registered last, or two users share it and one starts
    // receiving the other's notifications.
    //
    // One statement, not delete-then-insert. Two statements race: concurrent
    // registrations of the same token (a client retry, an app foregrounded
    // twice) both delete, then both insert, and the loser hits the
    // `UNIQUE (platform, token)` index and 500s on what is an idempotent
    // operation. Worse, the delete commits alone — so a failing insert leaves
    // the device deregistered and silently stops its push until the client
    // happens to try again.
    //
    // The upsert is also what finally makes `last_seen_at` mean something: a
    // re-register from the same device bumps it instead of writing a new row,
    // so it stops being a copy of `created_at` and starts being what prunes
    // tokens a device has stopped refreshing.
    upsert_device(
        &db,
        user.id,
        &body.platform,
        &token,
        body.device_name.as_deref(),
        now,
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    prune_devices(&db, user.id).await;

    info!(user = %user.id, "device registered for push");
    Ok(StatusCode::NO_CONTENT)
}

/// Claim `(platform, token)` for `user_id`, as one statement.
///
/// Separated from the handler so the semantics the comment above argues for —
/// idempotent, and ownership moves to the latest registrant — are testable
/// without an authenticated request.
pub async fn upsert_device(
    db: &sea_orm::DatabaseConnection,
    user_id: Uuid,
    platform: &str,
    token: &str,
    device_name: Option<&str>,
    now: chrono::DateTime<chrono::FixedOffset>,
) -> Result<(), sea_orm::DbErr> {
    device_tokens::Entity::insert(device_tokens::ActiveModel {
        id: Set(Uuid::new_v4()),
        user_id: Set(user_id),
        platform: Set(platform.to_string()),
        token: Set(token.to_string()),
        device_name: Set(device_name.map(str::to_string)),
        created_at: Set(now),
        last_seen_at: Set(now),
    })
    .on_conflict(
        sea_orm::sea_query::OnConflict::columns([
            device_tokens::Column::Platform,
            device_tokens::Column::Token,
        ])
        .update_columns([
            device_tokens::Column::UserId,
            device_tokens::Column::DeviceName,
            device_tokens::Column::LastSeenAt,
        ])
        .to_owned(),
    )
    .exec(db)
    .await
    .map(|_| ())
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

/// `GET /api/notifications/vapid-public-key` — the key a browser subscribes with.
///
/// # Why this route has to exist
///
/// `PushManager.subscribe` takes an `applicationServerKey`, and it is the VAPID
/// public key this deployment signs with. There was no way to obtain it: the
/// value is read from `OXY_VAPID_PUBLIC_KEY` inside the sender and never left
/// it. So a deployment could have a fully working Web Push sender and no
/// possible subscriber — the sender shipped, and nothing could register to be
/// sent to.
///
/// Answers `configured: false` rather than 404 when the environment is not set
/// up. A client asking this is deciding whether to offer a "turn on
/// notifications" control at all, and "this deployment does not do push" is an
/// answer it can act on, where a 404 is indistinguishable from an old build.
///
/// Authenticated but not otherwise gated: the key is published to the push
/// service on every send and is useless without the private half, so serving it
/// discloses nothing. It is behind auth only because everything on this router
/// is, and an unauthenticated probe for "is push configured" is a fingerprint
/// worth not handing out for free.
///
/// # What is and is not tested
///
/// The key's FORMAT is already pinned by
/// `web_push::tests::a_valid_subject_and_public_key_get_as_far_as_the_private_key`,
/// which proves a base64url uncompressed P-256 point is accepted by reaching
/// the private-key arm with a bogus PEM. This route adds no transform, and it
/// cannot be unit-tested past that: `WebPush` cannot be constructed without a
/// real private key, so there is no instance to call `public_key()` on. A first
/// draft of a test here asserted `is_err()`, which held whether the key was
/// accepted or rejected — it would have passed for the bug it was written to
/// catch — and tightening it only reproduced the existing test. Deleted rather
/// than shipped as a weaker duplicate.
#[derive(Serialize)]
pub struct VapidKeyResponse {
    pub configured: bool,
    /// Base64url, unpadded — the form `applicationServerKey` expects.
    pub public_key: Option<String>,
}

pub async fn vapid_public_key(
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
) -> Json<VapidKeyResponse> {
    // Built per request rather than held in state, matching how the sender
    // itself is constructed. Reading three env vars is cheaper than the
    // round trip that carries the answer.
    let configured = super::web_push::WebPush::from_env();
    Json(VapidKeyResponse {
        configured: configured.is_some(),
        public_key: configured.map(|w| w.public_key().to_string()),
    })
}
