//! Assignment writes: who holds which position, where, under whom.
//!
//! `org_role_members` had readers and no writer from the day it shipped
//! (#3050) until this module. The rule this adds and the model does not: an
//! assignment names somebody with STANDING in the org — a member, or an
//! active frontline worker — which is the same standing the app access
//! settings accept, and is what lets a worker be rostered at all.
//!
//! Below the HTTP layer because two surfaces write assignments: `oxy-app`'s
//! `/orgs/{id}/assignments` routes and `oxy-api-frontline`'s enrolment. The
//! handlers stay with their routes; this takes a connection and nothing else.

use axum::http::StatusCode;
use chrono::Utc;
use entity::{locations, org_frontline_members, org_members, org_role_members, org_roles};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, ModelTrait, QueryFilter,
    Set,
};
use uuid::Uuid;

use super::dto::AssignmentSpec;
use super::is_unique_violation;
use crate::audit;

#[derive(Debug, thiserror::Error)]
pub enum AssignError {
    #[error("no such position in this org")]
    NoSuchRole,
    #[error("no such location in this org")]
    NoSuchLocation,
    #[error("a position held at a location needs a location")]
    LocationRequired,
    #[error("an org-wide position is not held at a location")]
    LocationForbidden,
    #[error("only an org member or an active frontline worker can hold a position")]
    NoStanding,
    #[error("the supervisor must be an org member or an active frontline worker")]
    NoSuchSupervisor,
    #[error("no such assignment in this org")]
    NotFound,
    #[error("database error: {0}")]
    Db(#[from] DbErr),
}

impl AssignError {
    pub fn status(&self) -> StatusCode {
        match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::BAD_REQUEST,
        }
    }
}

/// Standing to hold a position: an org member, or an active frontline
/// worker. Suspension takes a worker out here by the same column the login,
/// the roster and the directory read.
pub async fn has_standing(
    db: &DatabaseConnection,
    org_id: Uuid,
    user_id: Uuid,
) -> Result<bool, DbErr> {
    let member = org_members::Entity::find()
        .filter(org_members::Column::OrgId.eq(org_id))
        .filter(org_members::Column::UserId.eq(user_id))
        .one(db)
        .await?
        .is_some();
    if member {
        return Ok(true);
    }
    Ok(org_frontline_members::Entity::find_by_id((org_id, user_id))
        .one(db)
        .await?
        .is_some_and(|w| w.status == org_frontline_members::STATUS_ACTIVE))
}

/// Everything about an assignment that can be checked before the person
/// exists: the position, the place, the scope rule, and the supervisor.
/// Enrolment calls this before it creates the worker, so a bad request never
/// leaves a half-set-up person behind.
pub async fn validate_targets(
    db: &DatabaseConnection,
    org_id: Uuid,
    spec: &AssignmentSpec,
) -> Result<org_roles::Model, AssignError> {
    let role = org_roles::Entity::find_by_id(spec.role_id)
        .filter(org_roles::Column::OrgId.eq(org_id))
        .one(db)
        .await?
        .ok_or(AssignError::NoSuchRole)?;
    match (role.is_location_scoped(), spec.location_id) {
        (true, None) => return Err(AssignError::LocationRequired),
        (false, Some(_)) => return Err(AssignError::LocationForbidden),
        (true, Some(location_id)) => {
            let present = locations::Entity::find_by_id(location_id)
                .filter(locations::Column::OrgId.eq(org_id))
                .one(db)
                .await?
                .is_some();
            if !present {
                return Err(AssignError::NoSuchLocation);
            }
        }
        (false, None) => {}
    }
    if let Some(supervisor) = spec.supervisor_id {
        if !has_standing(db, org_id, supervisor).await? {
            return Err(AssignError::NoSuchSupervisor);
        }
    }
    Ok(role)
}

pub struct Assigned {
    pub row: org_role_members::Model,
    /// False when the same (position, person, place) was already held; the
    /// row returned is the existing one. Idempotent by design — an import
    /// re-run must not duplicate a roster.
    pub created: bool,
    /// True when the position was already held and its supervisor changed:
    /// the one thing a re-post of the same position can mean, and it must
    /// not be dropped on the floor with a 200.
    pub updated: bool,
}

async fn find_existing(
    db: &DatabaseConnection,
    org_id: Uuid,
    user_id: Uuid,
    spec: &AssignmentSpec,
) -> Result<Option<org_role_members::Model>, DbErr> {
    let q = org_role_members::Entity::find()
        .filter(org_role_members::Column::OrgId.eq(org_id))
        .filter(org_role_members::Column::UserId.eq(user_id))
        .filter(org_role_members::Column::RoleId.eq(spec.role_id));
    let q = match spec.location_id {
        Some(l) => q.filter(org_role_members::Column::LocationId.eq(l)),
        None => q.filter(org_role_members::Column::LocationId.is_null()),
    };
    q.one(db).await
}

pub async fn assign(
    db: &DatabaseConnection,
    org_id: Uuid,
    user_id: Uuid,
    spec: &AssignmentSpec,
) -> Result<Assigned, AssignError> {
    validate_targets(db, org_id, spec).await?;
    if !has_standing(db, org_id, user_id).await? {
        return Err(AssignError::NoStanding);
    }
    if let Some(row) = find_existing(db, org_id, user_id, spec).await? {
        if row.supervisor_id == spec.supervisor_id {
            return Ok(Assigned {
                row,
                created: false,
                updated: false,
            });
        }
        let mut am: org_role_members::ActiveModel = row.into();
        am.supervisor_id = Set(spec.supervisor_id);
        let row = am.update(db).await?;
        return Ok(Assigned {
            row,
            created: false,
            updated: true,
        });
    }
    let inserted = org_role_members::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        role_id: Set(spec.role_id),
        user_id: Set(user_id),
        location_id: Set(spec.location_id),
        supervisor_id: Set(spec.supervisor_id),
        created_at: Set(Utc::now().fixed_offset()),
    }
    .insert(db)
    .await;
    match inserted {
        Ok(row) => Ok(Assigned {
            row,
            created: true,
            updated: false,
        }),
        Err(e) if is_unique_violation(&e) => {
            // Lost a race with an identical write; answer with the winner.
            let row = find_existing(db, org_id, user_id, spec)
                .await?
                .ok_or(AssignError::Db(e))?;
            Ok(Assigned {
                row,
                created: false,
                updated: false,
            })
        }
        Err(e) => Err(e.into()),
    }
}

/// Enrolment's half: every spec, in order, for a person who now exists — and
/// an audit entry per position gained, the same one the assignments route
/// files, so this door leaves the same trail.
pub async fn roster_at_enrolment(
    db: &DatabaseConnection,
    org_id: Uuid,
    user_id: Uuid,
    specs: &[AssignmentSpec],
    actor: &crate::audit::RequestActor,
) -> Result<Vec<Uuid>, AssignError> {
    let mut ids = Vec::with_capacity(specs.len());
    for spec in specs {
        let assigned = assign(db, org_id, user_id, spec).await?;
        if let Some(action) = audit_action(&assigned) {
            audit::record_best_effort(db, entry(actor, org_id, &assigned.row, action)).await;
        }
        ids.push(assigned.row.id);
    }
    Ok(ids)
}

pub async fn remove(
    db: &DatabaseConnection,
    org_id: Uuid,
    id: Uuid,
) -> Result<Option<org_role_members::Model>, DbErr> {
    let Some(row) = org_role_members::Entity::find_by_id(id)
        .filter(org_role_members::Column::OrgId.eq(org_id))
        .one(db)
        .await?
    else {
        return Ok(None);
    };
    row.clone().delete(db).await?;
    Ok(Some(row))
}

/// What a write did, for the trail: nothing for an identical re-post.
pub fn audit_action(assigned: &Assigned) -> Option<&'static str> {
    if assigned.created {
        Some("org.assignment.created")
    } else if assigned.updated {
        Some("org.assignment.updated")
    } else {
        None
    }
}

pub fn entry(
    actor: &crate::audit::RequestActor,
    org_id: Uuid,
    row: &org_role_members::Model,
    action: &'static str,
) -> audit::AuditEntry {
    audit::AuditEntry::for_request(actor, action)
        .org(org_id)
        .target("assignment", row.id.to_string(), row.user_id.to_string())
        .metadata(serde_json::json!({
            "user_id": row.user_id,
            "role_id": row.role_id,
            "location_id": row.location_id,
            "supervisor_id": row.supervisor_id,
        }))
}
