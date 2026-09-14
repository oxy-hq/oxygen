//! The two shelves above the folder tree: **Favorites** and **Pinned**.
//!
//! One module for both because they are the same product idea seen from two
//! authorities, and keeping them apart would hide the one thing a reader needs
//! to know about either:
//!
//! * **Favoriting is a personal act on something you can already read.** It is
//!   gated by [`super::handlers::readable`], not by a ring — a frontline worker
//!   bookmarking the sanitiser SOP is exactly who the feature is for, and they
//!   hold no `org_members` row.
//! * **Pinning is an officer saying "everybody read this".** It is gated by
//!   `OrgAdmin` and stored on the document, because it is a property of the
//!   document rather than of the viewer.
//!
//! If both had been per-user they would be one feature wearing two tab names,
//! and the Pinned tab would have nothing of its own to show.

use axum::extract::Path;
use axum::http::StatusCode;
use chrono::Utc;
use entity::{document_favorites, documents};
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::sea_query::{Expr, SimpleExpr};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use tracing::instrument;
use uuid::Uuid;

use super::handlers::{db_err, readable};
use super::manage::owned_document;
use crate::server::api::middlewares::role_guards::OrgAdmin;

/// `POST /api/documents/{id}/favorite`
///
/// Idempotent by construction: the composite primary key makes a second
/// favorite a no-op the database settles, so a double-tap on a tablet is not an
/// error the handler has to recognise.
#[instrument(skip_all, fields(document = %id))]
pub async fn favorite(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    // You may bookmark what you may read, and nothing else. `readable` answers
    // 404 for everything else, so a favorite cannot be used to probe whether a
    // document exists.
    readable(&db, user.id, id).await?;

    // `ON CONFLICT DO NOTHING` rather than read-then-insert. The doc above
    // promises the database settles a double tap; a `find` followed by an
    // `insert` promises it and does not deliver, because two taps that both
    // read "absent" both insert and the loser gets a unique violation the
    // handler renders as `500`. On a shared tablet a double tap is the normal
    // case, not the edge one.
    add_favorite(&db, user.id, id).await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Record the bookmark, idempotently.
///
/// Exported for the same reason `favorited_by` is: the test that proves a
/// double tap is not an error has to run THIS statement, not a copy of it. The
/// first version of that test re-implemented the insert inline, so a
/// regression in the handler would have left it green — which is the exact
/// anti-pattern the comment on `favorited_by` was written about, repeated four
/// lines away from it.
///
/// `on_conflict_do_nothing_on` returns a `TryInsert`, and that is load-bearing:
/// a plain `.exec()` on an insert that inserted nothing answers
/// `DbErr::RecordNotInserted`, which the handler renders as the `500` the
/// conflict clause exists to remove. `Insert::do_nothing` is deprecated in
/// sea-orm 2.0 for precisely this confusion, and this shipped using it once.
pub async fn add_favorite(
    db: &DatabaseConnection,
    caller: Uuid,
    document_id: Uuid,
) -> Result<(), sea_orm::DbErr> {
    document_favorites::Entity::insert(document_favorites::ActiveModel {
        user_id: Set(caller),
        document_id: Set(document_id),
        created_at: Set(Utc::now().fixed_offset()),
    })
    .on_conflict_do_nothing_on([
        document_favorites::Column::UserId,
        document_favorites::Column::DocumentId,
    ])
    .exec(db)
    .await?;
    Ok(())
}

/// "the caller has bookmarked this", as SQL.
///
/// Exported so the Favorites filter and the test that proves it cannot widen a
/// worker's library are the SAME predicate. They were two: the handler built an
/// `IN (...)` from every bookmark the caller had ever made, and the test built
/// its own copy of that — so the test could keep passing across a change to the
/// thing it was meant to guard.
///
/// `EXISTS` rather than a materialised id list: the result is clamped to a page
/// either way, and this version answers from `document_favorites`' primary key
/// without building the intermediate set at all.
pub fn favorited_by(caller: Uuid) -> SimpleExpr {
    Expr::cust_with_values(
        r#"EXISTS (SELECT 1 FROM document_favorites f
                    WHERE f.document_id = documents.id AND f.user_id = $1)"#,
        [caller],
    )
}

/// `DELETE /api/documents/{id}/favorite`
///
/// Deliberately does NOT go through `readable`. Un-favoriting a document you
/// have lost access to is the one moment you most need it to work — otherwise a
/// stale bookmark is stuck on a shelf forever, and the row is the caller's own.
#[instrument(skip_all, fields(document = %id))]
pub async fn unfavorite(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    document_favorites::Entity::delete_by_id((user.id, id))
        .exec(&db)
        .await
        .map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/orgs/{org_id}/documents/{id}/pin`
#[instrument(skip_all, fields(org = %org_id, document = %id))]
pub async fn pin(
    OrgAdmin(_ctx): OrgAdmin,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path((org_id, id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, StatusCode> {
    set_pin(org_id, id, Some(user.id)).await
}

/// `DELETE /api/orgs/{org_id}/documents/{id}/pin`
#[instrument(skip_all, fields(org = %org_id, document = %id))]
pub async fn unpin(
    OrgAdmin(_ctx): OrgAdmin,
    Path((org_id, id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, StatusCode> {
    set_pin(org_id, id, None).await
}

/// The only writer of the pin pair. Both columns move together or the schema
/// refuses the row, so there is one place that can get it wrong and it is here.
async fn set_pin(org_id: Uuid, id: Uuid, by: Option<Uuid>) -> Result<StatusCode, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let existing = owned_document(&db, org_id, id).await?;

    let now = Utc::now().fixed_offset();
    let mut am: documents::ActiveModel = existing.into();
    am.pinned_at = Set(by.map(|_| now));
    am.pinned_by = Set(by);
    am.updated_at = Set(now);
    am.update(&db).await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}
