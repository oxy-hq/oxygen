//! Turning ids into the names a screen shows.
//!
//! A document row carries `created_by`, `location_id` and `current_version_id`.
//! A folder card reads "12 items · Operations". Nothing in between is
//! interesting, and all of it is the same mistake waiting to be made twice: a
//! per-row lookup inside a loop, which turns one listing into ninety queries.
//!
//! So every lookup here is batched, and the batching is the reason this is a
//! module rather than four closures in `handlers.rs`. Listing a folder of
//! thirty documents costs five queries no matter how many rows come back.
//!
//! # Which write handlers use it, and which do not
//!
//! `create_document` does not: a row that has just been created has no author
//! name to resolve that the caller did not just supply, no version, no
//! category and no star, so the bare conversion is the whole truth for it.
//!
//! `update_document`, `update_folder` and `review::decide` DO, and the reason
//! is the one the original version of this comment missed. Those three return a
//! row the caller is about to RE-RENDER, and the bare conversion's defaults are
//! not neutral — `is_favorite: false` and null names are claims. Editing a
//! document you had starred came back unstarred; approving one blanked its
//! category tab label and its type badge. One indexed lookup for one row is
//! cheaper than a screen that disagrees with itself.

use std::collections::{HashMap, HashSet};

use entity::{
    document_categories, document_favorites, document_versions, documents, folders, locations,
    users,
};
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QuerySelect};
use uuid::Uuid;

use super::dto::{DocumentSummary, FolderNode};
use super::visibility::{ReadStanding, visible_documents};

/// Everything one page of documents needs, resolved once.
#[derive(Default)]
pub struct Names {
    users: HashMap<Uuid, String>,
    locations: HashMap<Uuid, String>,
    /// `current_version_id` → the version's number and content type. The kind a
    /// screen shows ("SOP", "PDF", "Chapter") is a rendering of the two, not a
    /// column — see [`DocumentSummary::content_type`].
    versions: HashMap<Uuid, (i32, Option<String>)>,
    categories: HashMap<Uuid, String>,
    /// The caller's own bookmarks. A set rather than a map: the only question
    /// asked of it is membership.
    favorites: HashSet<Uuid>,
}

/// Resolve the names for a page of rows.
///
/// **Five** queries at most, whatever the page size — users (authors and
/// reviewers together), locations, versions, categories, and the caller's own
/// favorites. Four of them are skipped entirely when no row on the page uses
/// the field; the favorites read is unconditional, because "not starred" is a
/// fact about the caller that absence cannot express.
pub async fn names_for(
    db: &DatabaseConnection,
    caller: Uuid,
    rows: &[documents::Model],
) -> Result<Names, DbErr> {
    let mut out = Names::default();
    if rows.is_empty() {
        return Ok(out);
    }

    // Authors and reviewers in one lookup. Two `IN` lists over the same table
    // would be two round trips for one question — "what are these people
    // called" — and the sets overlap heavily in practice, because the officer
    // who uploads a permit is usually the one who approves it.
    let user_ids: Vec<Uuid> = dedup(
        rows.iter()
            .filter_map(|d| d.created_by)
            .chain(rows.iter().filter_map(|d| d.reviewed_by)),
    );
    if !user_ids.is_empty() {
        out.users = users::Entity::find()
            .filter(users::Column::Id.is_in(user_ids))
            .all(db)
            .await?
            .into_iter()
            .map(|u| (u.id, u.name))
            .collect();
    }

    let location_ids: Vec<Uuid> = dedup(rows.iter().filter_map(|d| d.location_id));
    if !location_ids.is_empty() {
        out.locations = locations::Entity::find()
            .filter(locations::Column::Id.is_in(location_ids))
            .all(db)
            .await?
            .into_iter()
            .map(|l| (l.id, l.name))
            .collect();
    }

    let version_ids: Vec<Uuid> = dedup(rows.iter().filter_map(|d| d.current_version_id));
    if !version_ids.is_empty() {
        out.versions = document_versions::Entity::find()
            .filter(document_versions::Column::Id.is_in(version_ids))
            .all(db)
            .await?
            .into_iter()
            .map(|v| (v.id, (v.version_no, v.content_type)))
            .collect();
    }

    let category_ids: Vec<Uuid> = dedup(rows.iter().filter_map(|d| d.category_id));
    if !category_ids.is_empty() {
        out.categories = document_categories::Entity::find()
            .filter(document_categories::Column::Id.is_in(category_ids))
            .all(db)
            .await?
            .into_iter()
            .map(|c| (c.id, c.name))
            .collect();
    }

    // Only the ids on this page, not every bookmark the caller has ever made.
    // Scoped so a heavy user of Favorites does not make every listing heavier.
    let page: Vec<Uuid> = rows.iter().map(|d| d.id).collect();
    out.favorites = document_favorites::Entity::find()
        .filter(document_favorites::Column::UserId.eq(caller))
        .filter(document_favorites::Column::DocumentId.is_in(page))
        .all(db)
        .await?
        .into_iter()
        .map(|f| f.document_id)
        .collect();

    Ok(out)
}

fn dedup(it: impl Iterator<Item = Uuid>) -> Vec<Uuid> {
    let mut v: Vec<Uuid> = it.collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// One row, with its names filled in.
pub fn summary(d: documents::Model, names: &Names) -> DocumentSummary {
    let author_name = d.created_by.and_then(|id| names.users.get(&id).cloned());
    let location_name = d
        .location_id
        .and_then(|id| names.locations.get(&id).cloned());
    let current = d
        .current_version_id
        .and_then(|id| names.versions.get(&id).cloned());
    let category_name = d
        .category_id
        .and_then(|id| names.categories.get(&id).cloned());
    let is_favorite = names.favorites.contains(&d.id);
    let reviewed_by_name = d.reviewed_by.and_then(|id| names.users.get(&id).cloned());
    DocumentSummary {
        reviewed_by_name,
        is_favorite,
        category_name,
        author_name,
        location_name,
        version_no: current.as_ref().map(|(n, _)| *n),
        content_type: current.and_then(|(_, ct)| ct),
        ..DocumentSummary::from(d)
    }
}

/// A whole page, resolved and mapped.
pub async fn summaries(
    db: &DatabaseConnection,
    caller: Uuid,
    rows: Vec<documents::Model>,
) -> Result<Vec<DocumentSummary>, DbErr> {
    let names = names_for(db, caller, &rows).await?;
    Ok(rows.into_iter().map(|d| summary(d, &names)).collect())
}

/// Live document counts per folder, as the CALLER sees them, in one grouped
/// query.
///
/// This was deliberately org-wide, on the argument that a count is a property
/// of the folder and that two people disagreeing about it reads as data loss.
/// The argument does not survive contact with `hq`: an `org`-visible folder
/// holding three head-office documents reported "3 items" to a frontline
/// worker, who is the audience `hq` exists to exclude, and who then opened it
/// to an empty list. That is worse than a smaller number — it discloses that
/// head-office material exists and how much of it, and it renders a folder that
/// contradicts itself.
///
/// So the count is the number of documents this caller could actually open,
/// composed from the same `visible_documents` filter every listing uses rather
/// than a second implementation of the rule.
pub async fn folder_counts(
    db: &DatabaseConnection,
    org_id: Uuid,
    caller: Uuid,
    standing: &ReadStanding,
) -> Result<HashMap<Uuid, i64>, DbErr> {
    // No standing, no counts — and no query. Same branch every other read takes.
    let Some(visible) = visible_documents(org_id, caller, standing) else {
        return Ok(HashMap::new());
    };
    let rows: Vec<(Option<Uuid>, i64)> = documents::Entity::find()
        .select_only()
        .column(documents::Column::FolderId)
        .column_as(documents::Column::Id.count(), "n")
        .filter(visible)
        .filter(documents::Column::FolderId.is_not_null())
        .group_by(documents::Column::FolderId)
        .into_tuple()
        .all(db)
        .await?;

    Ok(rows
        .into_iter()
        .filter_map(|(id, n)| id.map(|id| (id, n)))
        .collect())
}

/// Folder rows with their counts attached.
pub fn folder_nodes(rows: Vec<folders::Model>, counts: &HashMap<Uuid, i64>) -> Vec<FolderNode> {
    rows.into_iter()
        .map(|f| FolderNode {
            item_count: counts.get(&f.id).copied().unwrap_or(0),
            id: f.id,
            parent_id: f.parent_id,
            name: f.name,
            visibility: f.visibility,
        })
        .collect()
}

/// Author names for a document's version history — the same batching, one
/// query, for the one field a history list shows that is not on its own row.
pub async fn author_names(
    db: &DatabaseConnection,
    rows: &[document_versions::Model],
) -> Result<HashMap<Uuid, String>, DbErr> {
    let ids = dedup(rows.iter().filter_map(|v| v.author_id));
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    Ok(users::Entity::find()
        .filter(users::Column::Id.is_in(ids))
        .all(db)
        .await?
        .into_iter()
        .map(|u| (u.id, u.name))
        .collect())
}

/// The current text of each document, for the ones handed in and no others.
///
/// Keyed by document id, and absent for a document that has no text — a file's
/// bytes live in object storage and never pass through this server, and a draft
/// may have no version at all. Absent therefore means "nothing to read here",
/// which is what a caller wants to know; it never means "not allowed", because
/// by the time anything calls this the rows have already passed
/// [`super::visibility::visible_documents`].
///
/// Scoped to `current_version_id` rather than to the document, deliberately.
/// Handing an engine every historical version would hand it text a reader has
/// to go looking for — the history is a record, not the document — and it would
/// grow with every edit for no gain in what the answer can say.
pub async fn bodies_of(
    db: &DatabaseConnection,
    documents: &[documents::Model],
) -> Result<HashMap<Uuid, String>, DbErr> {
    let wanted: Vec<(Uuid, Uuid)> = documents
        .iter()
        .filter_map(|d| d.current_version_id.map(|v| (v, d.id)))
        .collect();
    if wanted.is_empty() {
        return Ok(HashMap::new());
    }

    let by_version: HashMap<Uuid, Uuid> = wanted.iter().copied().collect();
    let rows = document_versions::Entity::find()
        .filter(document_versions::Column::Id.is_in(by_version.keys().copied().collect::<Vec<_>>()))
        .all(db)
        .await?;

    Ok(rows
        .into_iter()
        .filter_map(|v| {
            let doc = by_version.get(&v.id)?;
            v.body.map(|b| (*doc, b))
        })
        .collect())
}
