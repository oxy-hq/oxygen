//! Positions: renaming and retiring the `org_roles` vocabulary. Creating and
//! listing live in `work::handlers`, where the table was born.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use entity::{org_role_members, org_roles};
use oxy::database::client::establish_connection;
use oxy_app_core::audit;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, ModelTrait,
    PaginatorTrait, QueryFilter, Set,
};
use tracing::{instrument, warn};
use uuid::Uuid;

use super::dto::UpdateRole;
use super::{is_unique_violation, json_error};
use crate::server::api::middlewares::role_guards::OrgAdmin;

#[derive(Debug, thiserror::Error)]
pub enum RoleError {
    #[error("no such position in this org")]
    NotFound,
    #[error("a position needs a name")]
    BadName,
    #[error("another position in this org already has that name")]
    NameTaken,
    #[error("{0} people hold this position; move them first")]
    Held(u64),
    #[error("database error: {0}")]
    Db(#[from] DbErr),
}

impl RoleError {
    pub fn status(&self) -> StatusCode {
        match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::NameTaken | Self::Held(_) => StatusCode::CONFLICT,
            Self::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::BadName => StatusCode::BAD_REQUEST,
        }
    }
}

async fn in_org(
    db: &DatabaseConnection,
    org_id: Uuid,
    id: Uuid,
) -> Result<Option<org_roles::Model>, DbErr> {
    org_roles::Entity::find_by_id(id)
        .filter(org_roles::Column::OrgId.eq(org_id))
        .one(db)
        .await
}

pub async fn rename_role(
    db: &DatabaseConnection,
    org_id: Uuid,
    id: Uuid,
    name: &str,
) -> Result<org_roles::Model, RoleError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(RoleError::BadName);
    }
    let Some(row) = in_org(db, org_id, id).await? else {
        return Err(RoleError::NotFound);
    };
    let mut am: org_roles::ActiveModel = row.into();
    am.name = Set(name.to_string());
    am.updated_at = Set(Utc::now().fixed_offset());
    am.update(db).await.map_err(|e| {
        if is_unique_violation(&e) {
            RoleError::NameTaken
        } else {
            RoleError::Db(e)
        }
    })
}

/// Retire a position nobody holds. Held positions are refused rather than
/// cascaded: `org_role_members` cascades on delete, and silently emptying a
/// store's roster is not what "delete a label" means to anyone.
pub async fn delete_role(db: &DatabaseConnection, org_id: Uuid, id: Uuid) -> Result<(), RoleError> {
    let Some(row) = in_org(db, org_id, id).await? else {
        return Err(RoleError::NotFound);
    };
    let held = org_role_members::Entity::find()
        .filter(org_role_members::Column::RoleId.eq(id))
        .count(db)
        .await?;
    if held > 0 {
        return Err(RoleError::Held(held));
    }
    row.delete(db).await?;
    Ok(())
}

fn refused(e: RoleError) -> Response {
    if let RoleError::Db(err) = &e {
        warn!(error = %err, "position write failed");
    }
    json_error(e.status(), e.to_string())
}

/// `PATCH /api/orgs/{org_id}/roles/{id}` — org admin.
#[instrument(skip_all, fields(org = %org_id, role = %id))]
pub async fn patch_role(
    OrgAdmin(_ctx): OrgAdmin,
    actor: oxy_app_core::audit::RequestActor,
    Path((org_id, id)): Path<(Uuid, Uuid)>,
    Json(body): Json<UpdateRole>,
) -> Response {
    let Ok(db) = establish_connection().await else {
        return json_error(StatusCode::SERVICE_UNAVAILABLE, "database unavailable");
    };
    match rename_role(&db, org_id, id, &body.name).await {
        Ok(row) => {
            audit::record_best_effort(
                &db,
                audit::AuditEntry::for_request(&actor, "org.role.renamed")
                    .org(org_id)
                    .target("org_role", row.id.to_string(), row.name.clone()),
            )
            .await;
            Json(row).into_response()
        }
        Err(e) => refused(e),
    }
}

/// `DELETE /api/orgs/{org_id}/roles/{id}` — org admin. 409 while held.
#[instrument(skip_all, fields(org = %org_id, role = %id))]
pub async fn delete_role_handler(
    OrgAdmin(_ctx): OrgAdmin,
    actor: oxy_app_core::audit::RequestActor,
    Path((org_id, id)): Path<(Uuid, Uuid)>,
) -> Response {
    let Ok(db) = establish_connection().await else {
        return json_error(StatusCode::SERVICE_UNAVAILABLE, "database unavailable");
    };
    match delete_role(&db, org_id, id).await {
        Ok(()) => {
            audit::record_best_effort(
                &db,
                audit::AuditEntry::for_request(&actor, "org.role.deleted")
                    .org(org_id)
                    .target("org_role", id.to_string(), String::new()),
            )
            .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => refused(e),
    }
}
