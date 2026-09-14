//! Compliance's category tabs.
//!
//! A category is a label and a tab, not a permission. It never appears in
//! [`super::visibility`] and it never will — which is why this module can be
//! read end to end without wondering whether it decides access. It does not.
//!
//! Reading the list is open to anyone with standing in the org, because the
//! tabs have to render for the person looking at the table. Creating and
//! renaming go through `OrgAdmin` with everything else that shapes the library.

use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use chrono::Utc;
use entity::{document_categories, documents};
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set, SqlErr,
};
use std::collections::HashMap;
use tracing::instrument;
use uuid::Uuid;

use super::dto::{CategoryNode, CreateCategory, FolderQuery, UpdateCategory};
use super::handlers::db_err;
use super::visibility::{ReadStanding, resolve_standing, visible_documents};
use crate::server::api::middlewares::role_guards::OrgAdmin;

/// Is this the unique-name collision, rather than any other failure?
///
/// Two wrong versions preceded this one, and both are worth naming because the
/// second looked like a fix.
///
/// It began as `e.to_string().contains("23505")`, which matched nothing:
/// `DbErr`'s `Display` renders the driver's *message*, and the SQLSTATE is not
/// in it. Every duplicate answered `500`. The replacement added the English
/// sentence — `duplicate key value violates unique constraint` — which does
/// fire, on a server whose `lc_messages` is English. Point a deployment at one
/// that is not and the same `500` comes back, one config setting away, in a
/// check written specifically to prevent it.
///
/// `sql_err()` is the structured form and it unwraps `Query`, `Exec` and `Conn`
/// alike, which is the "depends on the sea-orm call" worry the old comment
/// raised, answered properly. `document_categories` has one meaningful unique
/// index — `(org_id, name)` — so "any unique violation" is exactly the
/// predicate, with nothing to disambiguate and nothing about locale to assume.
/// Same call this repo already makes at `crates/auth/src/user.rs` and, by
/// SQLSTATE, at `crates/app/src/server/api/org_teams/handlers.rs`.
pub fn is_duplicate_name(e: &DbErr) -> bool {
    matches!(e.sql_err(), Some(SqlErr::UniqueConstraintViolation(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The negative case, which is the half an integration test cannot reach:
    /// it can produce a real duplicate, but not a plausible non-duplicate error
    /// to prove the check discriminates. Both string versions of this function
    /// passed on the positive case too — that is why neither was caught.
    #[test]
    fn only_a_unique_violation_counts() {
        assert!(!is_duplicate_name(&DbErr::Custom(
            "duplicate key value violates unique constraint".into()
        )));
        assert!(!is_duplicate_name(&DbErr::RecordNotFound("nope".into())));
        assert!(!is_duplicate_name(&DbErr::Custom("23505".into())));
    }
}

/// Live documents per category, as the CALLER sees them.
///
/// This said "org-wide for the same reason folder counts are" and cited a
/// rationale that no longer exists one file over: `hydrate::folder_counts`
/// stopped being org-wide because an `org`-visible folder holding three
/// head-office documents reported "3 items" to the audience `hq` exists to
/// exclude. A category tab is the same disclosure with the same shape — it
/// reported `hq` documents, other stores' documents and unpublished drafts to a
/// frontline worker who then opens the tab and finds one row.
///
/// Same filter every listing composes, so the count and the list it labels
/// cannot disagree.
async fn counts(
    db: &DatabaseConnection,
    org_id: Uuid,
    caller: Uuid,
    standing: &ReadStanding,
) -> Result<HashMap<Uuid, i64>, DbErr> {
    let Some(visible) = visible_documents(org_id, caller, standing) else {
        return Ok(HashMap::new());
    };
    let rows: Vec<(Option<Uuid>, i64)> = documents::Entity::find()
        .select_only()
        .column(documents::Column::CategoryId)
        .column_as(documents::Column::Id.count(), "n")
        .filter(visible)
        .filter(documents::Column::CategoryId.is_not_null())
        .group_by(documents::Column::CategoryId)
        .into_tuple()
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, n)| id.map(|id| (id, n)))
        .collect())
}

/// `GET /api/document-categories?org_id=`
#[instrument(skip_all, fields(org = %q.org_id))]
pub async fn list(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Query(q): Query<FolderQuery>,
) -> Result<Json<Vec<CategoryNode>>, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;

    // Standing, not a ring: the same `404` every other read gives an org the
    // caller has nothing to do with, so a category list cannot be used to
    // enumerate a tenant's filing system.
    let standing = resolve_standing(&db, user.id, q.org_id)
        .await
        .map_err(db_err)?;
    if matches!(standing, ReadStanding::None) {
        return Err(StatusCode::NOT_FOUND);
    }

    let counts = counts(&db, q.org_id, user.id, &standing)
        .await
        .map_err(db_err)?;
    // Tabs across the top of a screen, so this is bounded by what fits there
    // long before it is bounded by anything else — but bounded in the query
    // rather than by hoping.
    let rows = document_categories::Entity::find()
        .filter(document_categories::Column::OrgId.eq(q.org_id))
        .order_by_asc(document_categories::Column::Name)
        .limit(500)
        .all(&db)
        .await
        .map_err(db_err)?;

    Ok(Json(
        rows.into_iter()
            .map(|c| CategoryNode {
                item_count: counts.get(&c.id).copied().unwrap_or(0),
                id: c.id,
                name: c.name,
            })
            .collect(),
    ))
}

/// `POST /api/orgs/{org_id}/document-categories`
#[instrument(skip_all, fields(org = %org_id))]
pub async fn create(
    OrgAdmin(_ctx): OrgAdmin,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(org_id): Path<Uuid>,
    Json(body): Json<CreateCategory>,
) -> Result<(StatusCode, Json<CategoryNode>), StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let now = Utc::now().fixed_offset();
    let saved = document_categories::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        name: Set(name),
        created_by: Set(Some(user.id)),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    }
    .insert(&db)
    .await
    // A name already taken in this org is a `409`, not a `500`. The unique
    // index is the only thing that makes "add category" safe under two people
    // clicking at once, and the caller needs to be told which of the two
    // happened.
    .map_err(|e| {
        if is_duplicate_name(&e) {
            StatusCode::CONFLICT
        } else {
            db_err(e)
        }
    })?;

    Ok((
        StatusCode::CREATED,
        Json(CategoryNode {
            id: saved.id,
            name: saved.name,
            item_count: 0,
        }),
    ))
}

/// `PATCH /api/orgs/{org_id}/document-categories/{id}` — rename.
#[instrument(skip_all, fields(org = %org_id, category = %id))]
pub async fn rename(
    OrgAdmin(_ctx): OrgAdmin,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path((org_id, id)): Path<(Uuid, Uuid)>,
    Json(body): Json<UpdateCategory>,
) -> Result<Json<CategoryNode>, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let existing = owned(&db, org_id, id).await?;

    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut am: document_categories::ActiveModel = existing.into();
    am.name = Set(name);
    am.updated_at = Set(Utc::now().fixed_offset());
    let saved = am.update(&db).await.map_err(|e| {
        if is_duplicate_name(&e) {
            StatusCode::CONFLICT
        } else {
            db_err(e)
        }
    })?;

    let standing = resolve_standing(&db, user.id, org_id)
        .await
        .map_err(db_err)?;
    let counts = counts(&db, org_id, user.id, &standing)
        .await
        .map_err(db_err)?;
    Ok(Json(CategoryNode {
        item_count: counts.get(&saved.id).copied().unwrap_or(0),
        id: saved.id,
        name: saved.name,
    }))
}

/// `DELETE /api/orgs/{org_id}/document-categories/{id}`
///
/// The documents keep existing and become uncategorised. That is a real delete
/// rather than a soft one because a category holds nothing — no bytes, no
/// visibility, no history. Undoing it is retyping the name.
#[instrument(skip_all, fields(org = %org_id, category = %id))]
pub async fn delete(
    OrgAdmin(_ctx): OrgAdmin,
    Path((org_id, id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let existing = owned(&db, org_id, id).await?;
    document_categories::Entity::delete_by_id(existing.id)
        .exec(&db)
        .await
        .map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

/// The row, scoped to the org the extractor authorized for. `404` rather than
/// `403`, like every other cross-tenant refusal here.
async fn owned(
    db: &DatabaseConnection,
    org_id: Uuid,
    id: Uuid,
) -> Result<document_categories::Model, StatusCode> {
    document_categories::Entity::find_by_id(id)
        .filter(document_categories::Column::OrgId.eq(org_id))
        .one(db)
        .await
        .map_err(db_err)?
        .ok_or(StatusCode::NOT_FOUND)
}
