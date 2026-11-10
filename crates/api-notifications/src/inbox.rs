//! `GET /api/notifications` (a page of my inbox) and
//! `GET /api/notifications/{id}` (one item of it).
//!
//! Self-scoped: the filter is `user_id = me`, which is the authorization.
//! There is no org gate, deliberately — a frontline worker holds no org
//! membership by design and is precisely who overdue-work notifications are
//! for.

use axum::Json;
use axum::extract::{Path, Query};
use axum::http::{HeaderMap, StatusCode};
use entity::notifications;
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect,
};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use uuid::Uuid;

use super::filter::FilterParams;
use super::inbox_view::{InboxItem, Links};

const DEFAULT_PAGE: u64 = 50;
const MAX_PAGE: u64 = 100;

#[derive(Debug, Deserialize)]
pub struct InboxQuery {
    /// Unread only. The default, because that is what a badge and a dropdown
    /// both want, and the read tail is unbounded.
    #[serde(default = "yes")]
    pub unread_only: bool,
    pub limit: Option<u64>,
    /// An item id: the page of items older than it (the previous page's
    /// `next_before`).
    pub before: Option<Uuid>,
    pub org_id: Option<Uuid>,
    pub app_id: Option<Uuid>,
    pub source: Option<String>,
    pub category: Option<String>,
    pub kind: Option<String>,
    pub thread: Option<String>,
}

fn yes() -> bool {
    true
}

impl InboxQuery {
    pub fn filter(&self) -> FilterParams {
        FilterParams {
            org_id: self.org_id,
            app_id: self.app_id,
            source: self.source.clone(),
            category: self.category.clone(),
            kind: self.kind.clone(),
            thread: self.thread.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct InboxResponse {
    /// Unread within the filter, `before` aside.
    pub unread: u64,
    pub items: Vec<InboxItem>,
    /// The `before` for the next, older page; `null` on the last one.
    pub next_before: Option<Uuid>,
}

/// `GET /api/notifications`
#[instrument(skip_all)]
pub async fn list(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Query(q): Query<InboxQuery>,
    headers: HeaderMap,
) -> Result<Json<InboxResponse>, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    page(&db, &user, &q, &Links::for_request(&headers))
        .await
        .map(Json)
}

/// `GET /api/notifications/{id}` — one of mine, for a phone's notification
/// service extension that fetches what a content-free push was about.
#[instrument(skip_all, fields(id = %id))]
pub async fn one(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<InboxItem>, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    item(&db, &user, id, &Links::for_request(&headers))
        .await
        .map(Json)
}

/// A page of `user`'s inbox. Ordered newest first, ties broken by id, so the
/// `before` cursor never skips or repeats an item.
pub async fn page(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    q: &InboxQuery,
    links: &Links,
) -> Result<InboxResponse, StatusCode> {
    let filter = q.filter().resolve(db).await?;
    let unread = filter
        .apply(notifications::Entity::find(), user)
        .filter(notifications::Column::ReadAt.is_null())
        .count(db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let limit = q.limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE);
    let mut find = filter
        .apply(notifications::Entity::find(), user)
        .order_by_desc(notifications::Column::CreatedAt)
        .order_by_desc(notifications::Column::Id)
        .limit(limit + 1);
    if q.unread_only {
        find = find.filter(notifications::Column::ReadAt.is_null());
    }
    if let Some(before) = q.before {
        find = find.filter(older_than(db, user, before).await?);
    }
    let mut rows = find
        .all(db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let next_before = match rows.len() as u64 > limit {
        true => {
            rows.truncate(limit as usize);
            rows.last().map(|r| r.id)
        }
        false => None,
    };
    // What each one says is in its org's own store; the count above never
    // reads it.
    let mut items = super::inbox_view::items(db, rows)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    links.apply(&mut items);
    Ok(InboxResponse {
        unread,
        items,
        next_before,
    })
}

/// Items after `before` in the page's order. `400` for a cursor that is not
/// one of `user`'s items, whoever's it is: a cursor must not confirm that
/// somebody else's notification exists.
async fn older_than(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    before: Uuid,
) -> Result<Condition, StatusCode> {
    let anchor = notifications::Entity::find_by_id(before)
        .one(db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .filter(|n| n.user_id == user.id && user.reaches_org(n.org_id))
        .ok_or(StatusCode::BAD_REQUEST)?;
    Ok(Condition::any()
        .add(notifications::Column::CreatedAt.lt(anchor.created_at))
        .add(
            Condition::all()
                .add(notifications::Column::CreatedAt.eq(anchor.created_at))
                .add(notifications::Column::Id.lt(anchor.id)),
        ))
}

/// One of `user`'s notifications, `404` for anyone else's — and for one of
/// an org the request's token does not reach, which the list leaves out too.
pub async fn item(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    id: Uuid,
    links: &Links,
) -> Result<InboxItem, StatusCode> {
    let row = mine(db, user, id).await?;
    let mut items = super::inbox_view::items(db, vec![row])
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    links.apply(&mut items);
    items.pop().ok_or(StatusCode::NOT_FOUND)
}

/// The signal `id` when it is `user`'s and its org is reached; `404` otherwise.
pub async fn mine(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    id: Uuid,
) -> Result<notifications::Model, StatusCode> {
    notifications::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .filter(|n| super::handlers::visible_to(n, user))
        .ok_or(StatusCode::NOT_FOUND)
}
