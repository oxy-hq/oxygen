//! Locations: the hierarchy, and what each integration calls a place.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use entity::{location_external_ids as ext, locations};
use oxy::database::client::establish_connection;
use oxy_app_core::audit;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, ModelTrait, QueryFilter,
    QueryOrder, Set,
};
use std::collections::{BTreeMap, HashMap};
use tracing::{instrument, warn};
use uuid::Uuid;

use super::dto::{LocationRow, SetExternalId, UpdateLocation, is_location_status};
use super::{is_unique_violation, json_error};
use crate::server::api::middlewares::role_guards::{OrgAdmin, OrgMemberStrict};

#[derive(Debug, thiserror::Error)]
pub enum LocationError {
    #[error("no such location in this org")]
    NotFound,
    #[error("a location needs a name")]
    BadName,
    #[error("another location in this org already has that name")]
    NameTaken,
    #[error("status must be one of pre_launch, launching, open, archived, terminated")]
    BadStatus,
    #[error("timezone is not an IANA zone name")]
    BadTimezone,
    #[error("no such parent location in this org")]
    NoSuchParent,
    #[error("a location cannot be its own ancestor")]
    Cycle,
    #[error("system must be a short lowercase token, like `toast`")]
    BadSystem,
    #[error("an external id cannot be empty")]
    BadExternalId,
    #[error("another location in this org already carries that id")]
    ExternalIdTaken,
    #[error("database error: {0}")]
    Db(#[from] DbErr),
}

impl LocationError {
    pub fn status(&self) -> StatusCode {
        match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::ExternalIdTaken | Self::NameTaken => StatusCode::CONFLICT,
            Self::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::BAD_REQUEST,
        }
    }
}

pub fn to_row(l: locations::Model, external_ids: BTreeMap<String, String>) -> LocationRow {
    LocationRow {
        id: l.id,
        org_id: l.org_id,
        name: l.name,
        kind: l.kind,
        parent_id: l.parent_id,
        status: l.status,
        timezone: l.timezone,
        external_id: l.external_id,
        external_ids,
        created_at: l.created_at.to_rfc3339(),
        updated_at: l.updated_at.to_rfc3339(),
    }
}

/// Every location of the org with its external ids, name-sorted. Two bounded
/// reads: an operator has tens to hundreds of places, not millions.
pub async fn location_rows(
    db: &DatabaseConnection,
    org_id: Uuid,
) -> Result<Vec<LocationRow>, DbErr> {
    let rows = locations::Entity::find()
        .filter(locations::Column::OrgId.eq(org_id))
        .order_by_asc(locations::Column::Name)
        .all(db)
        .await?;
    let ids = ext::Entity::find()
        .filter(ext::Column::OrgId.eq(org_id))
        .all(db)
        .await?;
    let mut by_location: HashMap<Uuid, BTreeMap<String, String>> = HashMap::new();
    for e in ids {
        by_location
            .entry(e.location_id)
            .or_default()
            .insert(e.system, e.external_id);
    }
    Ok(rows
        .into_iter()
        .map(|l| {
            let ids = by_location.remove(&l.id).unwrap_or_default();
            to_row(l, ids)
        })
        .collect())
}

pub async fn one_row(
    db: &DatabaseConnection,
    org_id: Uuid,
    id: Uuid,
) -> Result<Option<LocationRow>, DbErr> {
    let Some(l) = in_org(db, org_id, id).await? else {
        return Ok(None);
    };
    let ids = ext::Entity::find()
        .filter(ext::Column::LocationId.eq(id))
        .all(db)
        .await?
        .into_iter()
        .map(|e| (e.system, e.external_id))
        .collect();
    Ok(Some(to_row(l, ids)))
}

async fn in_org(
    db: &DatabaseConnection,
    org_id: Uuid,
    id: Uuid,
) -> Result<Option<locations::Model>, DbErr> {
    locations::Entity::find_by_id(id)
        .filter(locations::Column::OrgId.eq(org_id))
        .one(db)
        .await
}

/// Would making `child` a child of `parent` close a loop? Walks up from
/// `parent`; the database cannot say "acyclic", so the writer has to.
pub async fn would_cycle(
    db: &DatabaseConnection,
    org_id: Uuid,
    child: Uuid,
    parent: Uuid,
) -> Result<bool, DbErr> {
    let mut cursor = Some(parent);
    let mut hops = 0;
    while let Some(id) = cursor {
        if id == child {
            return Ok(true);
        }
        hops += 1;
        if hops > 64 {
            // A chain that deep is a loop in all but name, or a hierarchy no
            // operator has. Refusing is the safe answer either way.
            return Ok(true);
        }
        cursor = in_org(db, org_id, id).await?.and_then(|l| l.parent_id);
    }
    Ok(false)
}

pub async fn update_location(
    db: &DatabaseConnection,
    org_id: Uuid,
    id: Uuid,
    patch: UpdateLocation,
) -> Result<locations::Model, LocationError> {
    let Some(row) = in_org(db, org_id, id).await? else {
        return Err(LocationError::NotFound);
    };
    let mut am: locations::ActiveModel = row.into();
    if let Some(name) = patch.name {
        let name = name.trim();
        if name.is_empty() {
            return Err(LocationError::BadName);
        }
        am.name = Set(name.to_string());
    }
    if let Some(status) = patch.status {
        if !is_location_status(&status) {
            return Err(LocationError::BadStatus);
        }
        am.status = Set(status);
    }
    if let Some(tz) = patch.timezone {
        if tz.parse::<chrono_tz::Tz>().is_err() {
            return Err(LocationError::BadTimezone);
        }
        am.timezone = Set(tz);
    }
    if let Some(kind) = patch.kind {
        // A level is vocabulary; lowercased so "Region" and "region" are one
        // word in every picker.
        am.kind = Set(kind
            .map(|k| k.trim().to_lowercase())
            .filter(|k| !k.is_empty()));
    }
    if let Some(parent) = patch.parent_id {
        am.parent_id = Set(check_parent(db, org_id, id, parent).await?);
    }
    if let Some(external_id) = patch.external_id {
        am.external_id = Set(check_external_id(db, org_id, id, external_id).await?);
    }
    am.updated_at = Set(Utc::now().fixed_offset());
    am.update(db).await.map_err(|e| {
        if is_unique_violation(&e) {
            LocationError::NameTaken
        } else {
            LocationError::Db(e)
        }
    })
}

/// The tenant's own id a location may carry: trimmed, `None` for blank, and
/// held by no other location of the org — an app keys its places by it
/// (`santa-clara` in Store Ops), so two places sharing one would be one place
/// to the app. `id` is the location being written, so re-sending its own id
/// is not a collision. The table has no unique index on this column
/// (`(org_id, name)` is the only one), which is why the check lives here.
pub async fn check_external_id(
    db: &DatabaseConnection,
    org_id: Uuid,
    id: Uuid,
    value: Option<String>,
) -> Result<Option<String>, LocationError> {
    let Some(value) = value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    else {
        return Ok(None);
    };
    let taken = locations::Entity::find()
        .filter(locations::Column::OrgId.eq(org_id))
        .filter(locations::Column::ExternalId.eq(value.clone()))
        .filter(locations::Column::Id.ne(id))
        .one(db)
        .await?
        .is_some();
    if taken {
        return Err(LocationError::ExternalIdTaken);
    }
    Ok(Some(value))
}

/// The parent a location may take: none, or one of this org's that is not
/// itself or a descendant.
pub async fn check_parent(
    db: &DatabaseConnection,
    org_id: Uuid,
    id: Uuid,
    parent: Option<Uuid>,
) -> Result<Option<Uuid>, LocationError> {
    let Some(pid) = parent else {
        return Ok(None);
    };
    if pid == id {
        return Err(LocationError::Cycle);
    }
    if in_org(db, org_id, pid).await?.is_none() {
        return Err(LocationError::NoSuchParent);
    }
    if would_cycle(db, org_id, id, pid).await? {
        return Err(LocationError::Cycle);
    }
    Ok(Some(pid))
}

/// Record what `system` calls this location. An upsert: re-mapping a store
/// after a POS migration is the common case, not a conflict.
pub async fn set_external_id(
    db: &DatabaseConnection,
    org_id: Uuid,
    location_id: Uuid,
    system: &str,
    external_id: &str,
    actor: Option<Uuid>,
) -> Result<ext::Model, LocationError> {
    if !ext::is_valid_system(system) {
        return Err(LocationError::BadSystem);
    }
    let value = external_id.trim();
    if value.is_empty() {
        return Err(LocationError::BadExternalId);
    }
    if in_org(db, org_id, location_id).await?.is_none() {
        return Err(LocationError::NotFound);
    }
    let now = Utc::now().fixed_offset();
    let existing = ext::Entity::find_by_id((location_id, system.to_string()))
        .one(db)
        .await?;
    let written = match existing {
        Some(row) => {
            let mut am: ext::ActiveModel = row.into();
            am.external_id = Set(value.to_string());
            am.set_by = Set(actor);
            am.set_at = Set(now);
            am.update(db).await
        }
        None => {
            ext::ActiveModel {
                org_id: Set(org_id),
                location_id: Set(location_id),
                system: Set(system.to_string()),
                external_id: Set(value.to_string()),
                set_by: Set(actor),
                set_at: Set(now),
            }
            .insert(db)
            .await
        }
    };
    written.map_err(|e| {
        if is_unique_violation(&e) {
            LocationError::ExternalIdTaken
        } else {
            LocationError::Db(e)
        }
    })
}

pub async fn remove_external_id(
    db: &DatabaseConnection,
    org_id: Uuid,
    location_id: Uuid,
    system: &str,
) -> Result<bool, LocationError> {
    if in_org(db, org_id, location_id).await?.is_none() {
        return Err(LocationError::NotFound);
    }
    let Some(row) = ext::Entity::find_by_id((location_id, system.to_string()))
        .one(db)
        .await?
    else {
        return Ok(false);
    };
    row.delete(db).await?;
    Ok(true)
}

// ── Handlers ────────────────────────────────────────────────────────────────

fn refused(e: LocationError) -> Response {
    if let LocationError::Db(err) = &e {
        warn!(error = %err, "location write failed");
    }
    json_error(e.status(), e.to_string())
}

/// `GET /api/orgs/{org_id}/locations` — any member.
#[instrument(skip_all, fields(org = %org_id))]
pub async fn list_locations(
    OrgMemberStrict(_ctx): OrgMemberStrict,
    Path(org_id): Path<Uuid>,
) -> Response {
    let Ok(db) = establish_connection().await else {
        return json_error(StatusCode::SERVICE_UNAVAILABLE, "database unavailable");
    };
    match location_rows(&db, org_id).await {
        Ok(rows) => Json(serde_json::json!({ "locations": rows })).into_response(),
        Err(e) => {
            warn!(error = %e, "location list failed");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "list failed")
        }
    }
}

/// `PATCH /api/orgs/{org_id}/locations/{id}` — org admin.
#[instrument(skip_all, fields(org = %org_id, location = %id))]
pub async fn patch_location(
    OrgAdmin(_ctx): OrgAdmin,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((org_id, id)): Path<(Uuid, Uuid)>,
    Json(patch): Json<UpdateLocation>,
) -> Response {
    let Ok(db) = establish_connection().await else {
        return json_error(StatusCode::SERVICE_UNAVAILABLE, "database unavailable");
    };
    let row = match update_location(&db, org_id, id, patch).await {
        Ok(row) => row,
        Err(e) => return refused(e),
    };
    audit::record_best_effort(
        &db,
        audit::AuditEntry::new(actor.label().to_string(), "org.location.updated")
            .actor(actor.id, audit::ActorType::User)
            .org(org_id)
            .target("location", row.id.to_string(), row.name.clone()),
    )
    .await;
    match one_row(&db, org_id, id).await {
        Ok(Some(row)) => Json(row).into_response(),
        _ => json_error(StatusCode::INTERNAL_SERVER_ERROR, "read back failed"),
    }
}

/// `PUT /api/orgs/{org_id}/locations/{id}/external-ids/{system}` — org admin.
#[instrument(skip_all, fields(org = %org_id, location = %id, system = %system))]
pub async fn put_external_id(
    OrgAdmin(_ctx): OrgAdmin,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((org_id, id, system)): Path<(Uuid, Uuid, String)>,
    Json(body): Json<SetExternalId>,
) -> Response {
    let Ok(db) = establish_connection().await else {
        return json_error(StatusCode::SERVICE_UNAVAILABLE, "database unavailable");
    };
    match set_external_id(&db, org_id, id, &system, &body.external_id, Some(actor.id)).await {
        Ok(row) => {
            audit::record_best_effort(
                &db,
                audit::AuditEntry::new(actor.label().to_string(), "org.location.external_id_set")
                    .actor(actor.id, audit::ActorType::User)
                    .org(org_id)
                    .target("location", id.to_string(), system.clone()),
            )
            .await;
            Json(serde_json::json!({
                "location_id": row.location_id,
                "system": row.system,
                "external_id": row.external_id,
            }))
            .into_response()
        }
        Err(e) => refused(e),
    }
}

/// `DELETE /api/orgs/{org_id}/locations/{id}/external-ids/{system}` — org admin.
#[instrument(skip_all, fields(org = %org_id, location = %id, system = %system))]
pub async fn delete_external_id(
    OrgAdmin(_ctx): OrgAdmin,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((org_id, id, system)): Path<(Uuid, Uuid, String)>,
) -> Response {
    let Ok(db) = establish_connection().await else {
        return json_error(StatusCode::SERVICE_UNAVAILABLE, "database unavailable");
    };
    match remove_external_id(&db, org_id, id, &system).await {
        Ok(true) => {
            audit::record_best_effort(
                &db,
                audit::AuditEntry::new(
                    actor.label().to_string(),
                    "org.location.external_id_removed",
                )
                .actor(actor.id, audit::ActorType::User)
                .org(org_id)
                .target("location", id.to_string(), system.clone()),
            )
            .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => json_error(StatusCode::NOT_FOUND, "no such external id"),
        Err(e) => refused(e),
    }
}
