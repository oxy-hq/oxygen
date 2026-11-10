use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use chrono::Utc;
use entity::notifications;
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::Serialize;
use tracing::instrument;
use uuid::Uuid;

use super::filter::FilterParams;

/// Whether `user` may see `row`: their own, in an org the request's token
/// reaches. Anything else answers 404, never 403 — somebody else's
/// notification must not be confirmed to exist by the shape of the refusal.
pub(super) fn visible_to(
    row: &notifications::Model,
    user: &oxy_auth::types::AuthenticatedUser,
) -> bool {
    row.user_id == user.id && user.reaches_org(row.org_id)
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
    // 404 rather than 403: somebody else's notification must not be confirmed
    // to exist by the shape of the refusal. Nor one of an org the request's
    // token does not reach, which the inbox does not list either.
    let row = super::inbox::mine(&db, &user, id).await?;
    set_read(&db, row)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Mark `row` read. Already read is a no-op rather than an error — a double
/// tap on a phone is the normal case, not a mistake worth surfacing.
pub async fn set_read(
    db: &sea_orm::DatabaseConnection,
    row: notifications::Model,
) -> Result<(), sea_orm::DbErr> {
    if row.read_at.is_some() {
        return Ok(());
    }
    let mut update: notifications::ActiveModel = row.into();
    update.read_at = Set(Some(Utc::now().fixed_offset()));
    update.update(db).await.map(|_| ())
}

/// `POST /api/notifications/read-all`
///
/// "All" has to be a deliberate scope, because this is a WRITE: clearing
/// another org's unread as a side effect of clearing this one's is not
/// something a user can undo. So it takes the inbox's own filter — org, app,
/// source, category, kind, thread — as QUERY parameters, not a body.
/// `Option<Json<T>>` resolves to `None` when extraction fails for ANY reason,
/// so a caller who meant to scope this write and got the header wrong would
/// silently clear everything instead; a malformed query is a 400.
#[instrument(skip_all)]
pub async fn mark_all_read(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Query(q): Query<FilterParams>,
) -> Result<StatusCode, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    read_all(&db, &user, &q)
        .await
        .map(|_| StatusCode::NO_CONTENT)
}

/// Mark read every unread notification of `user`'s within `filter`; how many.
/// "All" is all the request's token reaches, within the stated filter: an org
/// that blocked the token keeps its unread, as the inbox keeps it out of the
/// count.
pub async fn read_all(
    db: &sea_orm::DatabaseConnection,
    user: &oxy_auth::types::AuthenticatedUser,
    filter: &FilterParams,
) -> Result<u64, StatusCode> {
    let update = filter
        .resolve(db)
        .await?
        .apply(notifications::Entity::update_many(), user)
        .col_expr(
            notifications::Column::ReadAt,
            sea_orm::sea_query::Expr::value(Utc::now().fixed_offset()),
        )
        .filter(notifications::Column::ReadAt.is_null());
    update
        .exec(db)
        .await
        .map(|r| r.rows_affected)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
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

#[cfg(test)]
#[path = "handlers_tests.rs"]
mod tests;
