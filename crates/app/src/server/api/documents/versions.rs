//! Adding a version.
//!
//! Two shapes behind one route, because the difference is where the bytes go
//! and nothing else:
//!
//! * A **chapter** arrives with its markdown. The row is written and becomes
//!   current in the same request — there is nothing else to wait for.
//! * A **file** gets a presigned PUT. The row is written immediately but does
//!   NOT become current; the browser uploads straight to the object store, then
//!   confirms. The bytes never pass through this server, which on a phone in a
//!   kitchen is the difference between an upload and a timeout.
//!
//! The version row is written before the upload rather than at confirm time on
//! purpose. It means the object key is chosen here, from ids this server
//! already holds, so confirm has no caller-supplied key to validate and no
//! chance to point a document at somebody else's object.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::Redirect;
use chrono::Utc;
use entity::{document_versions, documents};
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DbErr, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set,
    SqlErr,
};
use tracing::{instrument, warn};
use uuid::Uuid;

use super::dto::*;
use super::handlers::db_err;
use super::manage::owned_document;
use super::storage;
use crate::server::api::middlewares::role_guards::OrgAdmin;

/// Ceiling on an authored chapter.
///
/// 1 MB, raised from the 256 KB this shipped with. That number was picked for
/// markdown; chapters are rich text, and the same prose costs three to five
/// times as much once an editor has written the tags and attributes around it.
/// 256 KB of markdown is roughly a 40,000-word document, so this is the same
/// document's worth of HTML — still far past any SOP, still small enough that a
/// runaway paste cannot turn a row into a memory event on every reader's
/// request, and still comfortably inside a JSON response.
///
/// Anything genuinely larger is a file, and files have their own path.
const MAX_CHAPTER_BYTES: usize = 1024 * 1024;

/// How many revisions one history request returns, newest first.
///
/// A document's history is unbounded by construction — nothing prunes it — and
/// the screen reads it to choose a version, not to read two years of them.
const MAX_HISTORY: u64 = 100;

/// A lost race for a version number, told apart from a real failure.
///
/// `(document_id, version_no)` is unique, so a concurrent writer that computed
/// the same `next` loses here. That is the index doing its job — the caller has
/// to be told to retry, which is a `409`, and not that the server broke, which
/// is what `db_err` was saying.
fn version_taken(e: DbErr) -> StatusCode {
    if matches!(e.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) {
        StatusCode::CONFLICT
    } else {
        db_err(e)
    }
}

/// `POST /api/orgs/{org_id}/documents/{id}/versions`
#[instrument(skip_all, fields(org = %org_id, document = %id))]
pub async fn create(
    OrgAdmin(_ctx): OrgAdmin,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path((org_id, id)): Path<(Uuid, Uuid)>,
    Json(body): Json<NewVersion>,
) -> Result<(StatusCode, Json<NewVersionResponse>), StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let doc = owned_document(&db, org_id, id).await?;

    // Read the highest number and add one. The unique `(document_id,
    // version_no)` index is what makes that safe under concurrency: a second
    // writer racing to the same number loses on the constraint rather than
    // silently overwriting the first.
    //
    // Losing is a `409`, not a `500`. The integrity half of that sentence was
    // always true and the reported half was not: `db_err` rendered the
    // constraint as a server fault, so two people pressing Upload on the same
    // document — or one person double-tapping on a tablet, which is the normal
    // case rather than the edge one — got an opaque `500` with nothing saying
    // to retry. Six concurrent creates measured `201 500 500 500 201 500`.
    //
    // Same shape as the fix for a duplicate category name: recognise the unique
    // violation with sea-orm's structured check rather than a status guess.
    let next = document_versions::Entity::find()
        .filter(document_versions::Column::DocumentId.eq(id))
        .order_by_desc(document_versions::Column::VersionNo)
        .limit(1)
        .one(&db)
        .await
        .map_err(db_err)?
        .map_or(1, |v| v.version_no + 1);

    let now = Utc::now().fixed_offset();
    match doc.kind.as_str() {
        "chapter" => {
            let text = body.body.ok_or(StatusCode::BAD_REQUEST)?;
            if text.len() > MAX_CHAPTER_BYTES {
                return Err(StatusCode::PAYLOAD_TOO_LARGE);
            }
            let saved = document_versions::ActiveModel {
                id: Set(Uuid::new_v4()),
                document_id: Set(id),
                version_no: Set(next),
                author_id: Set(Some(user.id)),
                body: Set(Some(text)),
                object_key: Set(None),
                content_type: Set(Some("text/markdown".to_string())),
                size_bytes: Set(None),
                created_at: Set(now),
                ..Default::default()
            }
            .insert(&db)
            .await
            .map_err(version_taken)?;

            make_current(&db, doc, saved.id).await?;
            Ok((
                StatusCode::CREATED,
                Json(NewVersionResponse {
                    document_id: id,
                    version_no: next,
                    upload_url: None,
                    object_key: None,
                    is_current: true,
                }),
            ))
        }
        _ => {
            let content_type = body.content_type.unwrap_or_default();
            let length = body.content_length.ok_or(StatusCode::BAD_REQUEST)?;
            if content_type.trim().is_empty() {
                return Err(StatusCode::BAD_REQUEST);
            }
            // Signed BEFORE the row is written. A failure here leaves nothing
            // behind; the other order leaves a version row pointing at an
            // object that was never even offered a URL to arrive at.
            let (url, key) = storage::upload_url(org_id, id, next, &content_type, length)
                .await
                .map_err(|e| {
                    warn!(error = %e, "presigning a document upload failed");
                    storage::status_for(&e)
                })?;

            document_versions::ActiveModel {
                id: Set(Uuid::new_v4()),
                document_id: Set(id),
                version_no: Set(next),
                author_id: Set(Some(user.id)),
                body: Set(None),
                object_key: Set(Some(key.clone())),
                content_type: Set(Some(content_type)),
                size_bytes: Set(Some(length as i64)),
                created_at: Set(now),
                ..Default::default()
            }
            .insert(&db)
            .await
            .map_err(version_taken)?;

            Ok((
                StatusCode::CREATED,
                Json(NewVersionResponse {
                    document_id: id,
                    version_no: next,
                    upload_url: Some(url),
                    object_key: Some(key),
                    // Deliberately not current yet. Nothing has been uploaded.
                    is_current: false,
                }),
            ))
        }
    }
}

/// `POST /api/orgs/{org_id}/documents/{id}/versions/{version_no}/confirm`
///
/// Makes an uploaded version the one readers get — after checking that the
/// object is really there. Confirm is otherwise a claim the client makes about
/// an upload nobody verified, and its failure mode is a published document that
/// answers `404` long after anyone is watching.
#[instrument(skip_all, fields(org = %org_id, document = %id, version = version_no))]
pub async fn confirm(
    OrgAdmin(_ctx): OrgAdmin,
    Path((org_id, id, version_no)): Path<(Uuid, Uuid, i32)>,
) -> Result<StatusCode, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let doc = owned_document(&db, org_id, id).await?;

    let version = document_versions::Entity::find()
        .filter(document_versions::Column::DocumentId.eq(id))
        .filter(document_versions::Column::VersionNo.eq(version_no))
        .one(&db)
        .await
        .map_err(db_err)?
        .ok_or(StatusCode::NOT_FOUND)?;

    // A chapter is current the moment it is written, so confirming one is a
    // client bug rather than a state to reach.
    let key = version.object_key.clone().ok_or(StatusCode::BAD_REQUEST)?;

    if !storage::object_exists(&key).await.map_err(|e| {
        warn!(error = %e, "checking an uploaded document object failed");
        StatusCode::SERVICE_UNAVAILABLE
    })? {
        // The upload never arrived, or arrived under a different key. `409`
        // rather than `404`: the version row exists and the caller may retry
        // the PUT, which is a different instruction from "this is gone".
        return Err(StatusCode::CONFLICT);
    }

    let version_id = version.id;
    make_current(&db, doc, version_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Point the document at a version. The only writer of `current_version_id`.
async fn make_current(
    db: &sea_orm::DatabaseConnection,
    doc: documents::Model,
    version_id: Uuid,
) -> Result<(), StatusCode> {
    let mut am: documents::ActiveModel = doc.into();
    am.current_version_id = Set(Some(version_id));
    am.updated_at = Set(Utc::now().fixed_offset());
    am.update(db).await.map_err(db_err)?;
    Ok(())
}

/// `GET /api/documents/{id}/versions` — the history, newest first.
///
/// A read, so it goes through the same gate as every other read rather than
/// through `OrgAdmin`: somebody who may open a document may see what it used to
/// say. That is also why it is mounted outside `/orgs/{org_id}` with its
/// siblings — a frontline worker reading an SOP is exactly who might need to
/// check whether it changed since their shift.
#[instrument(skip_all, fields(document = %id))]
pub async fn list(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<VersionSummary>>, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    // `readable` is the one gate, and it answers 404 for anything the caller
    // may not see — including a document in another tenant.
    let doc = super::handlers::readable(&db, user.id, id).await?;

    // A ceiling, like every other listing. A document's history is small today
    // and unbounded by construction — nothing prunes it, and a chapter edited
    // daily for two years is 700 rows shipped to draw a list of dates.
    let rows = document_versions::Entity::find()
        .filter(document_versions::Column::DocumentId.eq(id))
        .order_by_desc(document_versions::Column::VersionNo)
        .limit(MAX_HISTORY)
        .all(&db)
        .await
        .map_err(db_err)?;

    let authors = super::hydrate::author_names(&db, &rows)
        .await
        .map_err(db_err)?;

    Ok(Json(
        rows.into_iter()
            .map(|v| VersionSummary {
                is_current: Some(v.id) == doc.current_version_id,
                author_name: v.author_id.and_then(|a| authors.get(&a).cloned()),
                version_no: v.version_no,
                content_type: v.content_type,
                size_bytes: v.size_bytes,
                created_at: v.created_at,
            })
            .collect(),
    ))
}

/// `GET /api/documents/{id}/versions/{version_no}` — open one version.
///
/// The half of the history list that was missing. `list` above says a version
/// exists, who wrote it and when; this returns what is in it, which is the
/// thing a compliance officer opens a history to find out.
///
/// A chapter comes back with its text. A file does not: its bytes are in the
/// object store and never pass through this server, so `body` is absent and
/// [`download`] below hands out a presigned URL for them. That split is the
/// same one every other read here makes, and `content_type` is what tells the
/// two apart — never the null.
#[instrument(skip_all, fields(document = %id, version = version_no))]
pub async fn read(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path((id, version_no)): Path<(Uuid, i32)>,
) -> Result<Json<VersionContent>, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    // One gate, the same one the listing uses: `404` for anything this caller
    // may not see, including a document in another tenant. A version is not a
    // separate thing to authorize — it belongs to the document, and the
    // document decides.
    let doc = super::handlers::readable(&db, user.id, id).await?;
    let version = owned_version(&db, id, version_no).await?;

    let authors = super::hydrate::author_names(&db, std::slice::from_ref(&version))
        .await
        .map_err(db_err)?;

    Ok(Json(VersionContent {
        is_current: Some(version.id) == doc.current_version_id,
        author_name: version.author_id.and_then(|a| authors.get(&a).cloned()),
        version_no: version.version_no,
        content_type: version.content_type,
        size_bytes: version.size_bytes,
        created_at: version.created_at,
        body: version.body,
    }))
}

/// `GET /api/documents/{id}/versions/{version_no}/download`
///
/// The bytes of ONE version, not of the current one. `handlers::download`
/// serves whatever is current, which is right for the button on a document and
/// useless for a history — it hands back the same file whichever row you click.
#[instrument(skip_all, fields(document = %id, version = version_no))]
pub async fn download(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path((id, version_no)): Path<(Uuid, i32)>,
) -> Result<Redirect, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let doc = super::handlers::readable(&db, user.id, id).await?;
    let version = owned_version(&db, id, version_no).await?;

    // A chapter has no object. `400` rather than `404` for the same reason the
    // document-level download does: the version is right there and readable,
    // the request is what is wrong, and saying so stops a client retrying it.
    let key = version.object_key.ok_or(StatusCode::BAD_REQUEST)?;

    // The version number in the filename, because a history download whose
    // three files are all called `Health Permit 2026.pdf` is three files nobody
    // can tell apart in a downloads folder.
    let titled = format!("{} (v{})", doc.title, version.version_no);
    let filename = super::handlers::with_extension(&titled, version.content_type.as_deref());

    let url = storage::download_url(&key, &filename).await.map_err(|e| {
        warn!(error = %e, "presigning a version download failed");
        storage::status_for(&e)
    })?;
    Ok(Redirect::temporary(&url))
}

/// One version of one document, or `404`.
///
/// Scoped by `document_id` as well as `version_no`, so a version number from
/// another document cannot be read through this document's authorization — the
/// pair is the identity, and the number alone is not unique across the table.
async fn owned_version(
    db: &sea_orm::DatabaseConnection,
    document_id: Uuid,
    version_no: i32,
) -> Result<document_versions::Model, StatusCode> {
    document_versions::Entity::find()
        .filter(document_versions::Column::DocumentId.eq(document_id))
        .filter(document_versions::Column::VersionNo.eq(version_no))
        .one(db)
        .await
        .map_err(db_err)?
        .ok_or(StatusCode::NOT_FOUND)
}
