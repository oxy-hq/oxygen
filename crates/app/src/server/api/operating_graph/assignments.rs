//! Assignments: who holds which position, where, under whom.
//!
//! `org_role_members` had readers and no writer from the day it shipped
//! (#3050) until this module. The rule this adds and the model does not: an
//! assignment names somebody with STANDING in the org — a member, or an
//! active frontline worker — which is the same standing the app access
//! settings accept, and is what lets a worker be rostered at all.

use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use oxy::database::client::establish_connection;
use oxy_app_core::audit;
use sea_orm::{DatabaseConnection, DbErr};
use tracing::{info, instrument, warn};
use uuid::Uuid;

use super::dto::{AssignmentRow, AssignmentsQuery, CreateAssignment};
use super::json_error;
use crate::server::api::middlewares::role_guards::{OrgAdmin, OrgMemberStrict};

// The writes — standing, target validation, `assign`, enrolment's roster and
// `remove` — live in `oxy_app_core::operating_graph::assignments`, which
// `oxy-api-frontline`'s enrolment shares. Re-exported so these handlers and
// the call sites in this crate keep their paths.
pub use oxy_app_core::operating_graph::assignments::*;

pub use oxy_app_core::operating_graph::readers::{by_user, rows};

async fn hydrate_one(
    db: &DatabaseConnection,
    org_id: Uuid,
    user_id: Uuid,
    id: Uuid,
) -> Result<Option<AssignmentRow>, DbErr> {
    let filter = AssignmentsQuery {
        user_id: Some(user_id),
        location_id: None,
    };
    Ok(rows(db, org_id, &filter)
        .await?
        .into_iter()
        .find(|r| r.id == id))
}

// ── Handlers ────────────────────────────────────────────────────────────────

fn refused(e: AssignError) -> Response {
    if let AssignError::Db(err) = &e {
        warn!(error = %err, "assignment write failed");
    }
    json_error(e.status(), e.to_string())
}

/// `GET /api/orgs/{org_id}/assignments?user_id&location_id` — any member.
#[instrument(skip_all, fields(org = %org_id))]
pub async fn list(
    OrgMemberStrict(_ctx): OrgMemberStrict,
    Path(org_id): Path<Uuid>,
    Query(filter): Query<AssignmentsQuery>,
) -> Response {
    let Ok(db) = establish_connection().await else {
        return json_error(StatusCode::SERVICE_UNAVAILABLE, "database unavailable");
    };
    match rows(&db, org_id, &filter).await {
        Ok(rows) => Json(serde_json::json!({ "assignments": rows })).into_response(),
        Err(e) => {
            warn!(error = %e, "assignment list failed");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "list failed")
        }
    }
}

/// `POST /api/orgs/{org_id}/assignments` — org admin. 201 when created, 200
/// when the person already held it.
#[instrument(skip_all, fields(org = %org_id, user = %req.user_id))]
pub async fn create(
    OrgAdmin(_ctx): OrgAdmin,
    actor: oxy_app_core::audit::RequestActor,
    Path(org_id): Path<Uuid>,
    Json(req): Json<CreateAssignment>,
) -> Response {
    let Ok(db) = establish_connection().await else {
        return json_error(StatusCode::SERVICE_UNAVAILABLE, "database unavailable");
    };
    let assigned = match assign(&db, org_id, req.user_id, &req.spec).await {
        Ok(a) => a,
        Err(e) => return refused(e),
    };
    if let Some(action) = audit_action(&assigned) {
        info!(assignment = %assigned.row.id, action, "assignment written");
        audit::record_best_effort(&db, entry(&actor, org_id, &assigned.row, action)).await;
    }
    match hydrate_one(&db, org_id, req.user_id, assigned.row.id).await {
        Ok(Some(row)) => {
            let status = if assigned.created {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };
            (status, Json(row)).into_response()
        }
        _ => json_error(StatusCode::INTERNAL_SERVER_ERROR, "read back failed"),
    }
}

/// `DELETE /api/orgs/{org_id}/assignments/{id}` — org admin.
#[instrument(skip_all, fields(org = %org_id, assignment = %id))]
pub async fn delete(
    OrgAdmin(_ctx): OrgAdmin,
    actor: oxy_app_core::audit::RequestActor,
    Path((org_id, id)): Path<(Uuid, Uuid)>,
) -> Response {
    let Ok(db) = establish_connection().await else {
        return json_error(StatusCode::SERVICE_UNAVAILABLE, "database unavailable");
    };
    match remove(&db, org_id, id).await {
        Ok(Some(row)) => {
            audit::record_best_effort(&db, entry(&actor, org_id, &row, "org.assignment.removed"))
                .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(None) => json_error(StatusCode::NOT_FOUND, AssignError::NotFound.to_string()),
        Err(e) => refused(AssignError::Db(e)),
    }
}
