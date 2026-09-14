//! Reading documents.
//!
//! Every handler here composes [`super::visibility::visible_documents`] and
//! nothing else decides who sees what. The writes live in
//! [`super::manage`], behind the `OrgAdmin` extractor.

use axum::Json;
use axum::extract::{OriginalUri, Path, Query};
use axum::http::StatusCode;
use axum::response::Redirect;
use entity::{document_versions, documents, folders};
use oxy::database::client::establish_connection;
use oxy_app_core::pagination::{self, Paged, trim_overfetch};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect,
};
use tracing::{instrument, warn};
use uuid::Uuid;

use super::dto::*;
use super::hydrate;
use super::storage;
use super::visibility::{
    Trash, resolve_standing, visible_documents_scoped, visible_folders_scoped,
};

/// The most folders one tree request returns.
const MAX_TREE: u64 = 2_000;

/// A page of documents is a screen, not an export. The ceiling is here rather
/// than trusted from the query string because a folder with ten thousand rows
/// is a memory event on the server before it is a slow page in the browser.
fn clamp_limit(requested: u64) -> u64 {
    requested.clamp(1, 500)
}

pub(super) fn db_err(e: DbErr) -> StatusCode {
    warn!(error = %e, "document read failed");
    StatusCode::INTERNAL_SERVER_ERROR
}

/// Fetch one document the caller is allowed to see, or `404`.
///
/// The refusal is `404` and never `403`, for every reason a read can fail:
/// wrong org, wrong store, an `hq` document, somebody else's draft. A `403`
/// would confirm that the document exists, which for a compliance library is
/// itself the leak — "there is a document you may not read, filed under
/// Litigation" is information.
pub(super) async fn readable(
    db: &DatabaseConnection,
    caller: Uuid,
    id: Uuid,
) -> Result<documents::Model, StatusCode> {
    // Loaded first only to learn which org to resolve standing in. Nothing
    // about the row is returned before the filter has been applied to it.
    let row = documents::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(db_err)?
        .ok_or(StatusCode::NOT_FOUND)?;

    let standing = resolve_standing(db, caller, row.org_id)
        .await
        .map_err(db_err)?;

    // BOTH sides of the trash, because `list` accepts `?deleted=true` and this
    // is what opens one of its rows. Gating on the live filter alone made every
    // row in the Deleted tab answer 404 when clicked — the tab could list a
    // document and then deny it existed, and restore was unreachable through
    // the UI that offers it.
    //
    // No policy is widened by the union: `Trash::Only` already degrades to the
    // live filter for anyone who is not an officer, so a frontline worker gets
    // the same condition twice and a trashed row still matches neither.
    let filter = [Trash::Excluded, Trash::Only]
        .into_iter()
        .filter_map(|t| visible_documents_scoped(row.org_id, caller, &standing, t))
        .fold(None::<Condition>, |acc, c| {
            Some(acc.unwrap_or_else(Condition::any).add(c))
        })
        .ok_or(StatusCode::NOT_FOUND)?;

    // Re-read THROUGH the filter rather than testing the row in memory. The
    // filter is SQL, and a second implementation of it in Rust is exactly the
    // drift `visibility` exists to prevent.
    documents::Entity::find_by_id(id)
        .filter(filter)
        .one(db)
        .await
        .map_err(db_err)?
        .ok_or(StatusCode::NOT_FOUND)
}

/// `GET /api/documents?org_id=&folder_id=&expiring_before=`
#[instrument(skip_all, fields(org = %q.org_id))]
pub async fn list(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    // `OriginalUri`, never a bare `Uri`. The router nests this surface under
    // `/api`, and axum hands an inner handler the path with that prefix already
    // stripped — so a `Link` built from `Uri` says `</documents?…>`, which a
    // client resolving it per RFC 3986 turns into `/documents?…` and gets a
    // 404. Every other adopter of `pagination::page` takes `OriginalUri` for
    // this reason; this handler did not, and a browser following the header
    // walked straight off the API.
    OriginalUri(uri): OriginalUri,
    Query(q): Query<ListQuery>,
) -> Result<Paged<DocumentSummary>, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;

    let standing = resolve_standing(&db, user.id, q.org_id)
        .await
        .map_err(db_err)?;
    let mut filter = visible_documents_scoped(
        q.org_id,
        user.id,
        &standing,
        if q.deleted {
            Trash::Only
        } else {
            Trash::Excluded
        },
    )
    .ok_or(StatusCode::NOT_FOUND)?;

    if let Some(folder) = q.folder_id {
        filter = filter.add(documents::Column::FolderId.eq(folder));
    }
    if q.favorited {
        // The caller's own bookmarks, intersected with what they may see. A
        // document favorited before it was moved out of reach stays bookmarked
        // and stops being listed, which is the correct half to lose.
        //
        // `super::shelf::favorited_by` rather than an expression built here,
        // so the test that proves this filter cannot widen a worker's library
        // is asserting on the predicate that actually ships.
        filter = filter.add(super::shelf::favorited_by(user.id));
    }
    if q.pinned {
        filter = filter.add(documents::Column::PinnedAt.is_not_null());
    }
    if let Some(cat) = q.category_id {
        filter = filter.add(documents::Column::CategoryId.eq(cat));
    }
    if let Some(review) = q.review_status.as_deref() {
        // Unvalidated on purpose: an unknown value matches nothing, which is
        // the honest answer to "show me the documents in a state that does not
        // exist". A 400 here would only be a different way to say zero.
        filter = filter.add(documents::Column::ReviewStatus.eq(review));
    }
    if let Some(horizon) = q.expiring_before {
        // Compliance's only listing filter. `is_not_null` is redundant against
        // the comparison but keeps the partial index applicable.
        filter = filter
            .add(documents::Column::ExpiresAt.is_not_null())
            .add(documents::Column::ExpiresAt.lte(horizon));
    }

    // The Pinned tab reads in the order an officer built the shelf; every other
    // listing reads newest-touched first.
    let mut query = documents::Entity::find().filter(filter);
    query = if q.pinned {
        // `id` here too. An officer pinning five documents in one sitting gives
        // them timestamps a millisecond apart at best, and the shelf is read
        // under the same page cap as everything else.
        query
            .order_by_desc(documents::Column::PinnedAt)
            .order_by_desc(documents::Column::Id)
    } else {
        // `id` breaks the tie. Two documents updated in the same millisecond
        // is the normal case for a bulk move or a seed run, and without a
        // tiebreaker the page they fall on is arbitrary.
        //
        // An earlier version of this comment said search "was just fixed for"
        // the same property. It had been given an ORDER BY and not a total
        // order; it has one now, and the two files agree.
        query
            .order_by_desc(documents::Column::UpdatedAt)
            .order_by_desc(documents::Column::Id)
    };
    // `limit + 1`: the extra row is how `rel="next"` below knows there is
    // another page, with no second `COUNT(*)` that could disagree with this
    // query because a document landed between the two. `trim_overfetch` drops
    // it again before anything sees it.
    let limit = clamp_limit(q.limit);
    let mut rows = query
        .offset(q.offset)
        .limit(limit + 1)
        .all(&db)
        .await
        .map_err(db_err)?;
    let more = trim_overfetch(&mut rows, limit);

    let items = hydrate::summaries(&db, user.id, rows)
        .await
        .map_err(db_err)?;

    // Routed through `page` even when this is the only page, so the response
    // always carries `rel="first"`. A paginated endpoint answering with NO
    // `Link` is byte-for-byte an endpoint that never paged, and a client cannot
    // tell whether it saw everything — which is exactly the state this
    // endpoint was in, and what made the app invent a `truncated` heuristic
    // from the row count.
    Ok(pagination::page(
        items,
        more,
        &uri,
        &[("offset", q.offset.saturating_add(limit).to_string())],
    ))
}

/// `GET /api/documents/{id}` — one document, with a chapter's text inline.
#[instrument(skip_all, fields(document = %id))]
pub async fn get(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(id): Path<Uuid>,
) -> Result<Json<DocumentDetail>, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let doc = readable(&db, user.id, id).await?;

    let current = match doc.current_version_id {
        Some(v) => document_versions::Entity::find_by_id(v)
            .one(&db)
            .await
            .map_err(db_err)?,
        None => None,
    };

    let names = hydrate::names_for(&db, user.id, std::slice::from_ref(&doc))
        .await
        .map_err(db_err)?;
    Ok(Json(DocumentDetail {
        // A file's bytes never come back inline — that is what the download
        // redirect is for, and inlining them would put a 100 MiB PDF through
        // the JSON encoder.
        body: current.and_then(|v| v.body),
        summary: hydrate::summary(doc, &names),
    }))
}

/// `GET /api/documents/{id}/download` — `307` to a short-lived presigned GET.
///
/// `307` rather than `302` because `Redirect::temporary` preserves the method,
/// which is the behaviour a download wants and the one a client can rely on.
///
/// The bytes never pass through this server. What cannot happen in the browser
/// is the signing, so this is the smallest possible privileged step: check the
/// filter, sign, redirect.
#[instrument(skip_all, fields(document = %id))]
pub async fn download(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(id): Path<Uuid>,
) -> Result<Redirect, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let doc = readable(&db, user.id, id).await?;

    // A draft with nothing uploaded yet has no current version, and there is
    // nothing to redirect to. `documents_published_has_a_version` means this
    // can only be a draft, which its author is allowed to see and not to
    // download.
    let version_id = doc.current_version_id.ok_or(StatusCode::NOT_FOUND)?;
    let version = document_versions::Entity::find_by_id(version_id)
        .one(&db)
        .await
        .map_err(db_err)?
        .ok_or(StatusCode::NOT_FOUND)?;

    // A chapter has no object to redirect to. `400` rather than `404`, because
    // the document is right there and readable — the request is the thing that
    // is wrong, and saying so is what stops a client retrying it forever.
    let key = version.object_key.ok_or(StatusCode::BAD_REQUEST)?;

    // Named with an extension, or the browser saves a file the operating
    // system cannot open. A document's title is prose — "Health Permit 2026" —
    // and the object's type is on the version, not in the title, so a download
    // arrived as an extensionless blob that Windows offered no application for.
    // Only appended when the title does not already end in the right one, so a
    // document somebody named `permit.pdf` does not become `permit.pdf.pdf`.
    let filename = with_extension(&doc.title, version.content_type.as_deref());

    let url = storage::download_url(&key, &filename).await.map_err(|e| {
        warn!(error = %e, "presigning a document download failed");
        storage::status_for(&e)
    })?;
    Ok(Redirect::temporary(&url))
}

/// The download filename: the title, plus the extension its content type
/// implies.
///
/// Deliberately a short allowlist rather than a mime database. These are the
/// types a compliance library actually holds, and an unknown type gets the bare
/// title — the same behaviour as before, for the cases where guessing would be
/// worse than not guessing.
pub(super) fn with_extension(title: &str, content_type: Option<&str>) -> String {
    let ext = match content_type.map(|c| c.split(';').next().unwrap_or(c).trim()) {
        Some("application/pdf") => "pdf",
        Some("image/png") => "png",
        Some("image/jpeg") => "jpg",
        Some("text/plain") => "txt",
        Some("text/csv") => "csv",
        Some("application/msword") => "doc",
        Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document") => "docx",
        Some("application/vnd.ms-excel") => "xls",
        Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet") => "xlsx",
        _ => return title.to_string(),
    };
    // Every spelling that means this type, not just the canonical one — a
    // document titled `photo.jpeg` was downloading as `photo.jpeg.jpg`.
    let already: &[&str] = match ext {
        "jpg" => &["jpg", "jpeg"],
        "txt" => &["txt", "text"],
        _ => &[],
    };
    let lower = title.to_ascii_lowercase();
    if lower.ends_with(&format!(".{ext}"))
        || already.iter().any(|e| lower.ends_with(&format!(".{e}")))
    {
        return title.to_string();
    }
    format!("{title}.{ext}")
}

#[cfg(test)]
mod tests {
    use super::with_extension;

    #[test]
    fn a_download_is_named_so_the_operating_system_can_open_it() {
        assert_eq!(
            with_extension("Health Permit 2026", Some("application/pdf")),
            "Health Permit 2026.pdf"
        );
        // Charset parameters are part of a content type and must not defeat it.
        assert_eq!(
            with_extension("Notes", Some("text/plain; charset=utf-8")),
            "Notes.txt"
        );
        // Already named for its type, in either case.
        assert_eq!(
            with_extension("permit.pdf", Some("application/pdf")),
            "permit.pdf"
        );
        assert_eq!(
            with_extension("permit.PDF", Some("application/pdf")),
            "permit.PDF"
        );
        // Already named for its type under a non-canonical spelling.
        assert_eq!(
            with_extension("holding.jpeg", Some("image/jpeg")),
            "holding.jpeg"
        );
        assert_eq!(
            with_extension("holding.JPEG", Some("image/jpeg")),
            "holding.JPEG"
        );
        // Unknown, and a chapter: better a bare title than a wrong extension.
        assert_eq!(
            with_extension("Handbook", Some("application/x-thing")),
            "Handbook"
        );
        assert_eq!(with_extension("Handbook", None), "Handbook");
    }
}

/// `GET /api/document-folders?org_id=` — the tree, flat.
#[instrument(skip_all, fields(org = %q.org_id))]
pub async fn list_folders(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Query(q): Query<FolderQuery>,
) -> Result<Json<Vec<FolderNode>>, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;

    let standing = resolve_standing(&db, user.id, q.org_id)
        .await
        .map_err(db_err)?;
    let filter = visible_folders_scoped(
        q.org_id,
        &standing,
        if q.deleted {
            Trash::Only
        } else {
            Trash::Excluded
        },
    )
    .ok_or(StatusCode::NOT_FOUND)?;

    // A ceiling. The tree is small in every tenant seen so far and unbounded in
    // the schema, and a folder listing is what the whole Knowledge screen is
    // built on — an org that made ten thousand of them should get a slow
    // screen, not a request that never returns.
    let rows = folders::Entity::find()
        .filter(filter)
        .order_by_asc(folders::Column::Name)
        .limit(MAX_TREE)
        .all(&db)
        .await
        .map_err(db_err)?;

    // A cap that is hit is worth saying out loud. The response is a bare array
    // — a wire contract this branch is not going to change at review time — so
    // the signal is a log line rather than a field, and it names the ceiling so
    // whoever reads it knows what to raise. Everything else in this product
    // surfaces a cap; this is the honest half-measure until the folder listing
    // gets the `Link`-header treatment `oxy_app_core::pagination` gives the
    // paginated routes.
    if rows.len() as u64 >= MAX_TREE {
        warn!(
            org = %q.org_id,
            cap = MAX_TREE,
            "document folder tree hit the listing cap — the client is seeing a truncated tree"
        );
    }

    let counts = hydrate::folder_counts(&db, q.org_id, user.id, &standing)
        .await
        .map_err(db_err)?;
    Ok(Json(hydrate::folder_nodes(rows, &counts)))
}
