//! What a manager does with the crew after enrolment: see them, decide which
//! apps each one opens, re-issue a forgotten PIN.
//!
//! Enrolment, standing and kiosks live in the `oxy-api-frontline` sibling crate
//! (`frontline` and `frontline_devices`), which also MOUNTS these handlers — one
//! path pairs `list_workers` with its `enrol`. This is the read-and-adjust
//! surface the org settings' Crew section is built on. Every route is
//! org-scoped under `OrgAdmin`; deciding an app's audience
//! is additionally `AppAccessManage` (`frontline_grants::may_grant_apps`), the
//! same ring the access settings enforce, so this door is not a way around
//! that one.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use entity::prelude::AppMembers;
use entity::{app_members, apps, org_frontline_members, user_credentials, users};
use oxy::database::client::establish_connection;
use oxy_app_core::audit;

use crate::server::api::operating_graph::assignments;
use crate::server::api::operating_graph::dto::WorkerAssignment;
use oxy_auth::frontline::{self, KIND_PIN, PinPolicy};
use oxy_shared::errors::OxyError;
use sea_orm::{
    ColumnTrait, DatabaseConnection, DbErr, EntityTrait, JoinType, QueryFilter, QuerySelect,
    RelationTrait,
};
use serde::{Deserialize, Serialize};
use tracing::{error, instrument, warn};
use uuid::Uuid;

use crate::server::api::frontline_grants::{self, GrantError};
use crate::server::api::middlewares::role_guards::OrgAdmin;

#[derive(Debug, Serialize)]
pub struct WorkerRow {
    pub user_id: Uuid,
    pub name: String,
    pub identifier: String,
    /// `active` | `suspended`.
    pub status: String,
    pub created_at: String,
    /// This org's apps the worker holds a grant on.
    pub apps: Vec<Uuid>,
    /// Set while the PIN lockout is in force — the state a manager sees when a
    /// worker says "it won't let me in" and the answer is a reset, not a wait.
    pub locked_until: Option<String>,
    /// Where they work, and as what: every position they hold.
    pub assignments: Vec<WorkerAssignment>,
}

/// Every worker enrolled in the org, with the facts a roster screen shows.
/// Four bounded reads, assembled in memory; a crew is tens to hundreds of
/// people, not millions.
pub async fn workers_of(db: &DatabaseConnection, org_id: Uuid) -> Result<Vec<WorkerRow>, DbErr> {
    let standing = org_frontline_members::Entity::find()
        .filter(org_frontline_members::Column::OrgId.eq(org_id))
        .all(db)
        .await?;
    if standing.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<Uuid> = standing.iter().map(|s| s.user_id).collect();
    let people = users::Entity::find()
        .filter(users::Column::Id.is_in(ids.clone()))
        .all(db)
        .await?;
    let creds = user_credentials::Entity::find()
        .filter(user_credentials::Column::Kind.eq(KIND_PIN))
        .filter(user_credentials::Column::OrgId.eq(Some(org_id)))
        .filter(user_credentials::Column::UserId.is_in(ids.clone()))
        .all(db)
        .await?;
    let grants = AppMembers::find()
        .join(JoinType::InnerJoin, app_members::Relation::Apps.def())
        .filter(apps::Column::OrgId.eq(org_id))
        .filter(app_members::Column::UserId.is_in(ids))
        .all(db)
        .await?;
    let mut held = assignments::by_user(db, org_id).await?;

    let mut rows: Vec<WorkerRow> = standing
        .into_iter()
        .map(|s| {
            let person = people.iter().find(|u| u.id == s.user_id);
            let cred = creds.iter().find(|c| c.user_id == s.user_id);
            let mut apps: Vec<Uuid> = grants
                .iter()
                .filter(|g| g.user_id == s.user_id)
                .map(|g| g.app_id)
                .collect();
            apps.sort_unstable();
            WorkerRow {
                user_id: s.user_id,
                name: person.map(|u| u.name.clone()).unwrap_or_default(),
                identifier: cred.map(|c| c.identifier.clone()).unwrap_or_default(),
                status: s.status,
                created_at: s.created_at.to_rfc3339(),
                apps,
                locked_until: cred
                    .and_then(|c| c.locked_until)
                    .filter(|t| *t > chrono::Utc::now())
                    .map(|t| t.to_rfc3339()),
                assignments: held.remove(&s.user_id).unwrap_or_default(),
            }
        })
        .collect();
    rows.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(rows)
}

#[derive(Debug, thiserror::Error)]
pub enum ReplaceError {
    #[error("no such worker in this org")]
    NotFound,
    #[error(transparent)]
    Grant(#[from] GrantError),
    #[error("database error: {0}")]
    Db(#[from] DbErr),
}

/// The outcome of a full replace, so the caller can audit what moved.
#[derive(Debug, Default)]
pub struct AppsChanged {
    pub added: Vec<apps::Model>,
    pub removed: Vec<apps::Model>,
    /// The worker's grants on this org's apps after the write, sorted.
    pub apps: Vec<Uuid>,
}

/// Make the worker's grants on THIS org's apps exactly `wanted`. Grants on
/// another org's apps — which a worker cannot hold anyway — are untouched;
/// an app already held keeps its role. Validation runs before any write.
pub async fn replace_worker_apps(
    db: &DatabaseConnection,
    org_id: Uuid,
    user_id: Uuid,
    wanted: &[Uuid],
    actor: Option<Uuid>,
) -> Result<AppsChanged, ReplaceError> {
    if org_frontline_members::Entity::find_by_id((org_id, user_id))
        .one(db)
        .await?
        .is_none()
    {
        return Err(ReplaceError::NotFound);
    }
    let wanted = frontline_grants::normalize_app_ids(wanted.to_vec());
    let wanted_rows = frontline_grants::validate_apps_in_org(db, org_id, &wanted).await?;

    let current: Vec<app_members::Model> = AppMembers::find()
        .join(JoinType::InnerJoin, app_members::Relation::Apps.def())
        .filter(apps::Column::OrgId.eq(org_id))
        .filter(app_members::Column::UserId.eq(user_id))
        .all(db)
        .await?;
    let held: Vec<Uuid> = current.iter().map(|g| g.app_id).collect();

    let to_add: Vec<Uuid> = wanted
        .iter()
        .copied()
        .filter(|a| !held.contains(a))
        .collect();
    let to_remove: Vec<Uuid> = held
        .iter()
        .copied()
        .filter(|a| !wanted.contains(a))
        .collect();

    let added = frontline_grants::grant_apps_to_worker(db, org_id, user_id, &to_add, actor).await?;
    let removed = if to_remove.is_empty() {
        Vec::new()
    } else {
        let rows = apps::Entity::find()
            .filter(apps::Column::Id.is_in(to_remove.clone()))
            .all(db)
            .await?;
        AppMembers::delete_many()
            .filter(app_members::Column::UserId.eq(user_id))
            .filter(app_members::Column::AppId.is_in(to_remove))
            .exec(db)
            .await?;
        crate::server::api::custom_apps_auth::invalidate_access_cache();
        rows
    };
    let mut apps: Vec<Uuid> = wanted_rows.iter().map(|a| a.id).collect();
    apps.sort_unstable();
    Ok(AppsChanged {
        added,
        removed,
        apps,
    })
}

fn json_error(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

// ── Handlers ────────────────────────────────────────────────────────────────

/// `GET /api/orgs/{org_id}/frontline/workers` — org admin.
#[instrument(skip_all, fields(org = %org_id))]
pub async fn list_workers(OrgAdmin(_ctx): OrgAdmin, Path(org_id): Path<Uuid>) -> Response {
    let Ok(db) = establish_connection().await else {
        return json_error(StatusCode::SERVICE_UNAVAILABLE, "database unavailable");
    };
    match workers_of(&db, org_id).await {
        Ok(workers) => Json(serde_json::json!({ "workers": workers })).into_response(),
        Err(e) => {
            error!(%org_id, "listing the crew failed: {e}");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "list failed")
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct WorkerAppsRequest {
    #[serde(default)]
    pub apps: Vec<Uuid>,
}

/// `PUT /api/orgs/{org_id}/frontline/workers/{user_id}/apps` — org admin
/// holding `AppAccessManage`. Full replace over this org's apps.
#[instrument(skip_all, fields(org = %org_id, worker = %user_id))]
pub async fn set_worker_apps(
    OrgAdmin(ctx): OrgAdmin,
    actor: oxy_app_core::audit::RequestActor,
    Path((org_id, user_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<WorkerAppsRequest>,
) -> Response {
    let Ok(db) = establish_connection().await else {
        return json_error(StatusCode::SERVICE_UNAVAILABLE, "database unavailable");
    };
    if !frontline_grants::may_grant_apps(&db, &crate::server::authz::caller_of(&actor), org_id)
        .await
    {
        return json_error(
            StatusCode::FORBIDDEN,
            "deciding a worker's apps needs the standing to manage app access",
        );
    }
    match replace_worker_apps(&db, org_id, user_id, &req.apps, Some(actor.id)).await {
        Ok(changed) => {
            // Gaining and losing access are both auditable: one entry per app
            // that moved, the same entry the access settings file.
            for (app, verb) in changed
                .added
                .iter()
                .map(|a| (a, "granted"))
                .chain(changed.removed.iter().map(|a| (a, "revoked")))
            {
                super::org_teams::audit::record(
                    &db,
                    &ctx,
                    &actor,
                    super::org_teams::audit::APP_ACCESS_CHANGED,
                    (
                        "app",
                        app.id,
                        format!("{} ({}) {verb} for worker {user_id}", app.name, app.slug),
                    ),
                )
                .await;
            }
            Json(serde_json::json!({ "apps": changed.apps })).into_response()
        }
        Err(ReplaceError::NotFound) => {
            json_error(StatusCode::NOT_FOUND, "no such worker in this org")
        }
        Err(ReplaceError::Grant(e @ GrantError::NotThisOrg(_))) => {
            json_error(StatusCode::BAD_REQUEST, e.to_string())
        }
        Err(e) => {
            error!(%org_id, %user_id, "replacing a worker's apps failed: {e}");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "update failed")
        }
    }
}

/// No `Debug`: holds a raw PIN.
#[derive(Deserialize)]
pub struct PinResetRequest {
    pub pin: String,
}

/// `POST /api/orgs/{org_id}/frontline/workers/{user_id}/pin` — org admin.
/// Re-issues the PIN and clears any lockout. 204; the PIN is never echoed.
#[instrument(skip_all, fields(org = %org_id, worker = %user_id))]
pub async fn reset_worker_pin(
    OrgAdmin(ctx): OrgAdmin,
    actor: oxy_app_core::audit::RequestActor,
    Path((org_id, user_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<PinResetRequest>,
) -> Response {
    let Ok(db) = establish_connection().await else {
        return json_error(StatusCode::SERVICE_UNAVAILABLE, "database unavailable");
    };
    match frontline::reset_pin(&db, org_id, user_id, &req.pin, PinPolicy::default()).await {
        Ok(()) => {
            // "Who re-issued this worker's credential, and when" — the same
            // trail suspension leaves. Best-effort, like its neighbours.
            audit::record_best_effort(
                &db,
                audit::AuditEntry::for_request(&actor, "frontline.worker.pin_reset")
                    .org(ctx.org.id)
                    .target("frontline_worker", user_id.to_string(), String::new()),
            )
            .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(OxyError::ValidationError(msg)) if msg == frontline::WORKER_NOT_FOUND => {
            json_error(StatusCode::NOT_FOUND, msg)
        }
        Err(OxyError::ValidationError(msg)) => json_error(StatusCode::BAD_REQUEST, msg),
        Err(e) => {
            warn!(%org_id, %user_id, "PIN reset failed: {e}");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "reset failed")
        }
    }
}
