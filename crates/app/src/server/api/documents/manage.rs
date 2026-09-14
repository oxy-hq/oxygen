//! Creating, editing and trashing documents and folders.
//!
//! Every handler here is behind the `OrgAdmin` extractor, and two different
//! tests hold that line — which is worth separating, because for a while one
//! sentence claimed both and neither did the second half.
//!
//! `manage_documents_ring_matches_the_shipped_guard` checks the MODEL: that
//! `Action::ManageDocuments` answers the same as a hand-written `Owner | Admin`
//! oracle across every scenario. It never reads a router or a signature, so it
//! cannot notice which extractor a handler takes.
//!
//! `every_org_scoped_document_write_takes_the_orgadmin_extractor`
//! (`tests/authz/document_write_guards.rs`) is the one that does. It scans this
//! module against the org-scoped router block, so mounting one of these behind
//! a different guard fails the build rather than shipping.
//!
//! # The extractor is not the whole gate
//!
//! `OrgAdmin` proves the caller is an officer of the org **on the path**. It
//! says nothing about the ids in the path segment after it, or in the body. A
//! document id from another tenant reaches this code with a valid extractor,
//! and the assignment graph shipped exactly that hole once. So every handler
//! re-reads its target scoped to `org_id`, and [`gate_refs`] checks the ids a
//! create or an update supplies.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use chrono::Utc;
use entity::{documents, folders, locations, org_role_members};
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use tracing::{instrument, warn};
use uuid::Uuid;

use super::dto::*;
use super::handlers::db_err;
use super::hydrate;
use super::visibility::Trash;
use crate::server::api::middlewares::role_guards::OrgAdmin;

fn valid_visibility(v: &str) -> bool {
    matches!(v, "org" | "hq")
}

fn valid_kind(k: &str) -> bool {
    matches!(k, "file" | "chapter")
}

fn valid_status(s: &str) -> bool {
    matches!(s, "draft" | "published")
}

/// A refusal a client can act on, or any other status unchanged.
///
/// `From<StatusCode>` is what lets a handler keep returning bare statuses for
/// everything else and `?` them through this type — so adding a reason to one
/// refusal did not mean inventing a body for every 404 and 503 beside it.
#[derive(Debug)]
pub struct Refusal {
    status: StatusCode,
    /// Machine-readable, and stable. Absent when the status is the whole story.
    code: Option<&'static str>,
    reason: Option<&'static str>,
}

impl Refusal {
    fn bad_request(code: &'static str, reason: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: Some(code),
            reason: Some(reason),
        }
    }
}

impl Refusal {
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// The machine-readable half, for a caller that branches on it — the client
    /// re-reads its folder tree on `folder_missing` — and for the gate tests,
    /// which pin THIS rather than the status: every refusal here is a `400`, so
    /// asserting the status says almost nothing about which rule fired.
    pub fn code(&self) -> Option<&'static str> {
        self.code
    }
}

impl From<StatusCode> for Refusal {
    fn from(status: StatusCode) -> Self {
        Self {
            status,
            code: None,
            reason: None,
        }
    }
}

impl axum::response::IntoResponse for Refusal {
    fn into_response(self) -> axum::response::Response {
        match (self.code, self.reason) {
            (Some(code), Some(reason)) => (
                self.status,
                Json(serde_json::json!({ "code": code, "reason": reason })),
            )
                .into_response(),
            // No body at all rather than an empty object: a client that reads
            // `reason` and finds nothing must fall back to its own wording, and
            // `{}` is a slower way of saying the same thing.
            _ => self.status.into_response(),
        }
    }
}

/// Every id a write supplies, checked against the org on the path.
///
/// Separated from the handlers because it IS the authorization, and an authz
/// decision reachable only through an extractor is one no test covers. The
/// assignment graph learned this the expensive way: its first cross-tenant fix
/// covered four of five ids.
///
/// Both refusals are `400` rather than `404`. The caller is already an officer
/// of the org on the path — they can see it exists — so a foreign folder id is
/// a fact about their request, not a boundary that has to stay unconfirmed.
///
/// # Every refusal here names itself
///
/// These used to be a bare `StatusCode::BAD_REQUEST` with no body, and a `400`
/// that says nothing forces the client to invent a cause. The one it invented
/// was "an empty file, or a document with nothing in it" — the only two 400s
/// its author knew about — so filing a chapter into a folder that had been
/// trashed in another tab reported an empty file, over a screen with the text
/// plainly on it. Both the person writing it and the person debugging it went
/// looking at object storage.
///
/// The reason is a stable machine-readable `code` plus a sentence. The code is
/// what a client may branch on — `folder_missing` means "your folder tree is
/// stale", which is a thing it can fix by re-reading rather than a thing to
/// report at somebody.
pub async fn gate_refs(
    db: &DatabaseConnection,
    org_id: Uuid,
    folder_id: Option<Uuid>,
    location_id: Option<Uuid>,
    category_id: Option<Uuid>,
) -> Result<(), Refusal> {
    if let Some(category) = category_id {
        let ok = entity::document_categories::Entity::find_by_id(category)
            .filter(entity::document_categories::Column::OrgId.eq(org_id))
            .one(db)
            .await
            .map_err(db_err)?
            .is_some();
        if !ok {
            warn!(%org_id, %category, "document write refused — category is not in this org");
            return Err(Refusal::bad_request(
                "category_missing",
                "that category is not in this organisation, or has been deleted",
            ));
        }
    }
    if let Some(folder) = folder_id {
        // A LIVE folder. Without the trash filter a document could be filed
        // into one that is in the bin — legal in the schema, and a state the
        // API would be creating on purpose-looking input: the document lands
        // somewhere no tree shows it, and only the "treat an unseeable folder
        // as top-level" rule stops it disappearing outright. Re-parenting a
        // folder under a trashed one is the same shape and the same filter.
        let ok = folders::Entity::find_by_id(folder)
            .filter(folders::Column::OrgId.eq(org_id))
            .filter(folders::Column::DeletedAt.is_null())
            .one(db)
            .await
            .map_err(db_err)?
            .is_some();
        if !ok {
            warn!(%org_id, %folder, "document write refused — folder is not in this org");
            return Err(Refusal::bad_request(
                "folder_missing",
                "that folder has been deleted, or is not in this organisation",
            ));
        }
    }
    // A place NOBODY IS ROSTERED AT, that also has children, cannot carry a
    // document.
    //
    // `visible_documents` matches `location_id` literally against the caller's
    // roster rows, so a document filed somewhere no roster points is invisible
    // to every frontline worker — a `201`, and a document unreadable by exactly
    // the people it is for.
    //
    // The first version of this guard refused any place with children, on the
    // stated ground that "no roster points at a container". That premise is
    // false: `operating_graph::assignments::validate_targets` accepts ANY
    // location in the org for a location-scoped role, with no leaf check — so a
    // District Manager rostered at "Northeast" is a supported state, and
    // refusing that write broke it. It also refused permanently for a store
    // that merely has an archived child row, which needs no flagship to hit.
    //
    // So the condition is the conjunction the reasoning actually supports: no
    // roster row AND children. A childless place with no roster is still
    // accepted — that is every store on its first day, and refusing it would
    // break opening one. A place with a roster is accepted whether or not it
    // has children, because somebody can read it.
    if let Some(location) = location_id {
        let rostered = org_role_members::Entity::find()
            .filter(org_role_members::Column::OrgId.eq(org_id))
            .filter(org_role_members::Column::LocationId.eq(location))
            .one(db)
            .await
            .map_err(db_err)?
            .is_some();
        let has_children = !rostered
            && locations::Entity::find()
                .filter(locations::Column::OrgId.eq(org_id))
                .filter(locations::Column::ParentId.eq(location))
                .one(db)
                .await
                .map_err(db_err)?
                .is_some();
        if has_children {
            warn!(
                %org_id, %location,
                "document write refused — that place has places under it and nobody rostered \
                 at it, so no frontline worker could read the document"
            );
            return Err(Refusal::bad_request(
                "location_unreachable",
                "that place has places under it and nobody rostered at it, so no store worker could read the document",
            ));
        }
    }

    if let Some(location) = location_id {
        let ok = locations::Entity::find_by_id(location)
            .filter(locations::Column::OrgId.eq(org_id))
            .one(db)
            .await
            .map_err(db_err)?
            .is_some();
        if !ok {
            warn!(%org_id, %location, "document write refused — location is not in this org");
            return Err(Refusal::bad_request(
                "location_missing",
                "that location is not in this organisation, or has been deleted",
            ));
        }
    }
    Ok(())
}

/// The target row, scoped to the org the extractor authorized for.
///
/// `404`, not `403`: a document in another tenant must not be confirmed to
/// exist by the shape of the refusal, even to an officer of some other org.
pub(super) async fn owned_document(
    db: &DatabaseConnection,
    org_id: Uuid,
    id: Uuid,
) -> Result<documents::Model, StatusCode> {
    documents::Entity::find_by_id(id)
        .filter(documents::Column::OrgId.eq(org_id))
        .one(db)
        .await
        .map_err(db_err)?
        .ok_or(StatusCode::NOT_FOUND)
}

/// The folder, scoped to the org — and to the LIVE side of the trash unless the
/// caller says otherwise.
///
/// `gate_refs` already refuses a trashed folder as a filing *destination*; the
/// subject was unguarded, so a folder in the bin could still be renamed,
/// re-parented and re-scoped. Restoring it then brought back something other
/// than what was trashed, and re-parenting one under a live folder put a
/// deleted container in the middle of a live tree.
///
/// `Trash::Only` is for `restore_folder`, which by definition operates on a
/// trashed row.
pub async fn owned_folder_scoped(
    db: &DatabaseConnection,
    org_id: Uuid,
    id: Uuid,
    trash: Trash,
) -> Result<folders::Model, StatusCode> {
    let mut q = folders::Entity::find_by_id(id).filter(folders::Column::OrgId.eq(org_id));
    q = match trash {
        Trash::Excluded => q.filter(folders::Column::DeletedAt.is_null()),
        Trash::Only => q.filter(folders::Column::DeletedAt.is_not_null()),
    };
    q.one(db)
        .await
        .map_err(db_err)?
        .ok_or(StatusCode::NOT_FOUND)
}

async fn owned_folder(
    db: &DatabaseConnection,
    org_id: Uuid,
    id: Uuid,
) -> Result<folders::Model, StatusCode> {
    owned_folder_scoped(db, org_id, id, Trash::Excluded).await
}

// ── Folders ─────────────────────────────────────────────────────────────────

/// `POST /api/orgs/{org_id}/document-folders`
#[instrument(skip_all, fields(org = %org_id))]
pub async fn create_folder(
    OrgAdmin(_ctx): OrgAdmin,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(org_id): Path<Uuid>,
    Json(body): Json<CreateFolder>,
) -> Result<(StatusCode, Json<FolderNode>), Refusal> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(Refusal::bad_request("name_empty", "a folder needs a name"));
    }
    let visibility = body.visibility.unwrap_or_else(|| "org".to_string());
    if !valid_visibility(&visibility) {
        return Err(Refusal::bad_request(
            "visibility_invalid",
            "audience must be everyone or head office only",
        ));
    }
    // A parent from another org would graft this org's tree onto a stranger's.
    gate_refs(&db, org_id, body.parent_id, None, None).await?;

    let now = Utc::now().fixed_offset();
    let saved = folders::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        parent_id: Set(body.parent_id),
        name: Set(name),
        visibility: Set(visibility),
        created_by: Set(Some(user.id)),
        created_at: Set(now),
        updated_at: Set(now),
        deleted_at: Set(None),
        ..Default::default()
    }
    .insert(&db)
    .await
    .map_err(db_err)?;

    Ok((
        StatusCode::CREATED,
        Json(FolderNode {
            // A folder is created empty, and renaming one does not move
            // documents. Both responses say 0 rather than paying for a count
            // the caller could not have changed.
            item_count: 0,
            id: saved.id,
            parent_id: saved.parent_id,
            name: saved.name,
            visibility: saved.visibility,
        }),
    ))
}

/// `PATCH /api/orgs/{org_id}/document-folders/{id}` — rename, move, re-scope.
#[instrument(skip_all, fields(org = %org_id, folder = %id))]
pub async fn update_folder(
    OrgAdmin(_ctx): OrgAdmin,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path((org_id, id)): Path<(Uuid, Uuid)>,
    Json(body): Json<UpdateFolder>,
) -> Result<Json<FolderNode>, Refusal> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let existing = owned_folder(&db, org_id, id).await?;

    let mut am: folders::ActiveModel = existing.into();
    if let Some(name) = body.name {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(Refusal::bad_request("name_empty", "a folder needs a name"));
        }
        am.name = Set(name);
    }
    if let Some(parent) = body.parent_id {
        // A folder cannot be its own parent, and cannot be moved under its own
        // descendant.
        //
        // The second half used to be left to "the tree is assembled", which
        // named code that does not exist: `hydrate::folder_nodes` returns a
        // flat list and every client walks down from `parent_id IS NULL`. So
        // `A.parent = B; B.parent = A` was accepted with a `200`, and both
        // folders plus everything filed in them became unreachable from the
        // root for every client at once — with no error, and no screen able to
        // show them again.
        if parent == Some(id) {
            return Err(Refusal::bad_request(
                "parent_is_self",
                "a folder cannot be filed inside itself",
            ));
        }
        if let Some(new_parent) = parent
            && is_descendant_of(&db, org_id, new_parent, id)
                .await
                .map_err(db_err)?
        {
            return Err(Refusal::bad_request(
                "parent_is_descendant",
                "a folder cannot be filed inside one of its own subfolders",
            ));
        }
        gate_refs(&db, org_id, parent, None, None).await?;
        am.parent_id = Set(parent);
    }
    if let Some(v) = body.visibility {
        if !valid_visibility(&v) {
            return Err(Refusal::bad_request(
                "visibility_invalid",
                "audience must be everyone or head office only",
            ));
        }
        am.visibility = Set(v);
    }
    am.updated_at = Set(Utc::now().fixed_offset());
    let saved = am.update(&db).await.map_err(db_err)?;

    // The real count, the way `categories::rename` does it. Returning 0 made a
    // renamed folder's card read "0 items" until the next full listing — the
    // rename looked like it had emptied the folder.
    // The officer's own view of the count. `OrgAdmin` has already established
    // they are one, so resolving standing here is a formality that keeps this
    // going through the same filter as every other count rather than assuming
    // an equivalence.
    let standing = super::visibility::resolve_standing(&db, user.id, org_id)
        .await
        .map_err(db_err)?;
    let counts = hydrate::folder_counts(&db, org_id, user.id, &standing)
        .await
        .map_err(db_err)?;

    Ok(Json(FolderNode {
        item_count: counts.get(&saved.id).copied().unwrap_or(0),
        id: saved.id,
        parent_id: saved.parent_id,
        name: saved.name,
        visibility: saved.visibility,
    }))
}

/// Is `candidate` at or below `root` in the folder tree?
///
/// Walks up from the candidate rather than down from the root: a folder has one
/// parent and any number of children, so upward is a single chain and bounded
/// by depth instead of by breadth.
///
/// The `seen` set is not decoration. If a cycle already exists in the data —
/// this shipped accepting them, so a tenant may have one — walking up would
/// loop forever, and a cycle check that hangs on the input it is checking for
/// is worse than none.
pub async fn is_descendant_of(
    db: &DatabaseConnection,
    org_id: Uuid,
    candidate: Uuid,
    root: Uuid,
) -> Result<bool, sea_orm::DbErr> {
    let mut seen = std::collections::HashSet::new();
    let mut at = Some(candidate);
    while let Some(id) = at {
        if id == root {
            return Ok(true);
        }
        if !seen.insert(id) {
            return Ok(false);
        }
        at = folders::Entity::find_by_id(id)
            .filter(folders::Column::OrgId.eq(org_id))
            .one(db)
            .await?
            .and_then(|f| f.parent_id);
    }
    Ok(false)
}

/// `DELETE /api/orgs/{org_id}/document-folders/{id}` — into the trash.
///
/// The documents inside are deliberately NOT trashed with it: deleting a
/// compliance library by tidying a folder is the worse of the two failures
/// available here.
///
/// The other one is real and this handler causes it, so it is stated rather
/// than claimed away. A document keeps its `folder_id` after its folder leaves
/// the listing, so a naive reader finds it under no folder AND not at the root
/// — it renders under nothing, and the only way to reach it is this API. It was
/// found that way: a library reporting twelve documents and showing eleven.
///
/// `folder_id` is deliberately still not cleared, because keeping it is what
/// lets `restore_folder` put every document back exactly where it was. **A
/// reader must therefore treat a document whose folder it cannot see as
/// top-level** — including a folder hidden by the viewer's own permissions
/// rather than by the trash. `contentsOf` in the Store Ops app does this; any
/// other client has to as well.
#[instrument(skip_all, fields(org = %org_id, folder = %id))]
pub async fn trash_folder(
    OrgAdmin(_ctx): OrgAdmin,
    Path((org_id, id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, StatusCode> {
    set_folder_deleted(org_id, id, Some(Utc::now().fixed_offset())).await
}

/// `POST /api/orgs/{org_id}/document-folders/{id}/restore`
#[instrument(skip_all, fields(org = %org_id, folder = %id))]
pub async fn restore_folder(
    OrgAdmin(_ctx): OrgAdmin,
    Path((org_id, id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, StatusCode> {
    set_folder_deleted(org_id, id, None).await
}

async fn set_folder_deleted(
    org_id: Uuid,
    id: Uuid,
    deleted_at: Option<chrono::DateTime<chrono::FixedOffset>>,
) -> Result<StatusCode, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    // Each direction looks on the side it can act on: trashing needs a live
    // folder, restoring needs a trashed one. Both were `owned_folder`, so
    // trashing an already-trashed folder answered `204` having changed only a
    // timestamp, and restoring a live one did the same.
    let side = if deleted_at.is_some() {
        Trash::Excluded
    } else {
        Trash::Only
    };
    let existing = owned_folder_scoped(&db, org_id, id, side).await?;
    let mut am: folders::ActiveModel = existing.into();
    am.deleted_at = Set(deleted_at);
    am.updated_at = Set(Utc::now().fixed_offset());
    am.update(&db).await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Documents ───────────────────────────────────────────────────────────────

/// `POST /api/orgs/{org_id}/documents` — always as a draft.
///
/// A document is created empty and published later, because publishing is what
/// makes it readable and there is nothing to read yet. The schema agrees:
/// `documents_published_has_a_version` makes "published with nothing in it"
/// unrepresentable rather than merely discouraged.
#[instrument(skip_all, fields(org = %org_id))]
pub async fn create_document(
    OrgAdmin(_ctx): OrgAdmin,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(org_id): Path<Uuid>,
    Json(body): Json<CreateDocument>,
) -> Result<(StatusCode, Json<DocumentSummary>), Refusal> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let title = body.title.trim().to_string();
    if title.is_empty() || !valid_kind(&body.kind) {
        return Err(Refusal::bad_request(
            "title_empty",
            "a document needs a title, and a kind of chapter or file",
        ));
    }
    let visibility = body.visibility.unwrap_or_else(|| "org".to_string());
    if !valid_visibility(&visibility) {
        return Err(Refusal::bad_request(
            "visibility_invalid",
            "audience must be everyone or head office only",
        ));
    }
    gate_refs(
        &db,
        org_id,
        body.folder_id,
        body.location_id,
        body.category_id,
    )
    .await?;

    let now = Utc::now().fixed_offset();
    let saved = documents::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        folder_id: Set(body.folder_id),
        category_id: Set(body.category_id),
        title: Set(title),
        kind: Set(body.kind),
        status: Set("draft".to_string()),
        visibility: Set(visibility),
        location_id: Set(body.location_id),
        expires_at: Set(body.expires_at),
        current_version_id: Set(None),
        created_by: Set(Some(user.id)),
        created_at: Set(now),
        updated_at: Set(now),
        deleted_at: Set(None),
        ..Default::default()
    }
    .insert(&db)
    .await
    .map_err(db_err)?;

    Ok((StatusCode::CREATED, Json(saved.into())))
}

/// `PATCH /api/orgs/{org_id}/documents/{id}` — move, re-scope, publish.
#[instrument(skip_all, fields(org = %org_id, document = %id))]
pub async fn update_document(
    OrgAdmin(_ctx): OrgAdmin,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path((org_id, id)): Path<(Uuid, Uuid)>,
    Json(body): Json<UpdateDocument>,
) -> Result<Json<DocumentSummary>, Refusal> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let existing = owned_document(&db, org_id, id).await?;

    // Publishing is the one transition with a precondition, and the schema
    // enforces it anyway. Checked here so the caller gets a 400 that says which
    // rule they broke instead of a 500 carrying a constraint name.
    if body.status.as_deref() == Some("published") && existing.current_version_id.is_none() {
        return Err(Refusal::bad_request(
            "publish_without_version",
            "this document has nothing in it yet, so there is nothing to publish",
        ));
    }
    gate_refs(
        &db,
        org_id,
        body.folder_id.flatten(),
        body.location_id.flatten(),
        body.category_id.flatten(),
    )
    .await?;

    let mut am: documents::ActiveModel = existing.into();
    if let Some(title) = body.title {
        let title = title.trim().to_string();
        if title.is_empty() {
            return Err(Refusal::bad_request(
                "title_empty",
                "a document needs a title",
            ));
        }
        am.title = Set(title);
    }
    if let Some(folder) = body.folder_id {
        am.folder_id = Set(folder);
    }
    if let Some(category) = body.category_id {
        am.category_id = Set(category);
    }
    if let Some(location) = body.location_id {
        am.location_id = Set(location);
    }
    if let Some(expires) = body.expires_at {
        am.expires_at = Set(expires);
    }
    if let Some(v) = body.visibility {
        if !valid_visibility(&v) {
            return Err(Refusal::bad_request(
                "visibility_invalid",
                "audience must be everyone or head office only",
            ));
        }
        am.visibility = Set(v);
    }
    if let Some(s) = body.status {
        if !valid_status(&s) {
            return Err(Refusal::bad_request(
                "status_invalid",
                "status must be draft or published",
            ));
        }
        am.status = Set(s);
    }
    am.updated_at = Set(Utc::now().fixed_offset());
    let saved = am.update(&db).await.map_err(db_err)?;

    // One row, so one indexed lookup rather than the batched path — but not the
    // `false` the bare conversion would ship. Editing a document you have
    // starred came back `is_favorite: false`, and a screen that trusts the
    // response un-draws the star until the next full listing.
    let names = hydrate::names_for(&db, user.id, std::slice::from_ref(&saved))
        .await
        .map_err(db_err)?;
    Ok(Json(hydrate::summary(saved, &names)))
}

/// `DELETE /api/orgs/{org_id}/documents/{id}`
#[instrument(skip_all, fields(org = %org_id, document = %id))]
pub async fn trash_document(
    OrgAdmin(_ctx): OrgAdmin,
    Path((org_id, id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, StatusCode> {
    set_document_deleted(org_id, id, Some(Utc::now().fixed_offset())).await
}

/// `POST /api/orgs/{org_id}/documents/{id}/restore`
#[instrument(skip_all, fields(org = %org_id, document = %id))]
pub async fn restore_document(
    OrgAdmin(_ctx): OrgAdmin,
    Path((org_id, id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, StatusCode> {
    set_document_deleted(org_id, id, None).await
}

async fn set_document_deleted(
    org_id: Uuid,
    id: Uuid,
    deleted_at: Option<chrono::DateTime<chrono::FixedOffset>>,
) -> Result<StatusCode, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let existing = owned_document(&db, org_id, id).await?;
    let mut am: documents::ActiveModel = existing.into();
    am.deleted_at = Set(deleted_at);
    am.updated_at = Set(Utc::now().fixed_offset());
    am.update(&db).await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}
