//! `/admin/apps/{app_id}/access` — Oxy staff editing a tenant's app audience.
//!
//! **Why this isn't just the org route.** `/organizations/{org_id}/*` requires a
//! real membership or a live assume-role session, and `block_admin_while_acting`
//! closes the whole of `/admin/*` the moment an operator starts acting. So a
//! Global Admin standing in the apps console can reach the org route only by
//! leaving the console. That is a deliberate guardrail, not an oversight — which
//! makes a staff-gated route the right answer rather than a workaround.
//!
//! Behavior is [`org_teams::service`], identical to the org's own surface; only the
//! gate differs. The app's `org_id` comes from the app row, so staff never name the
//! org — there is no way to point this at a tenant boundary it doesn't already own.

use axum::{Json, extract::Path, http::StatusCode};
use entity::prelude::Apps;
use oxy::database::client::establish_connection;
use sea_orm::EntityTrait;
use uuid::Uuid;

use crate::server::api::org_teams::dto::{
    AppAccessDto, OrgMemberOptionDto, SetAppAccessRequest, TeamDto,
};
use crate::server::api::org_teams::service;
use oxy_app_core::audit::{self, AuditEntry};

/// Resolve an app to the org that owns it.
///
/// The whole `/admin/apps` tree is already behind the staff guard (see
/// `admin::apps::router`), so reaching this function means the caller is Oxy staff;
/// this only turns an app id into its tenant.
async fn app_org(db: &sea_orm::DatabaseConnection, app_id: Uuid) -> Result<Uuid, StatusCode> {
    Apps::find_by_id(app_id)
        .one(db)
        .await
        .map_err(service::db_err)?
        .map(|a| a.org_id)
        .ok_or(StatusCode::NOT_FOUND)
}

/// `GET /admin/apps/{app_id}/access`
pub async fn get_app_access(Path(app_id): Path<Uuid>) -> Result<Json<AppAccessDto>, StatusCode> {
    let db = establish_connection().await.map_err(service::db_err)?;
    let org_id = app_org(&db, app_id).await?;
    let app = service::load_app_in_org(&db, org_id, app_id).await?;
    Ok(Json(service::read_access(&db, &app).await?))
}

/// `PUT /admin/apps/{app_id}/access`
pub async fn set_app_access(
    _: crate::server::api::custom_apps_agent_refusal::RefuseSandboxAgent,
    actor: oxy_app_core::audit::RequestActor,
    Path(app_id): Path<Uuid>,
    Json(req): Json<SetAppAccessRequest>,
) -> Result<Json<AppAccessDto>, StatusCode> {
    let db = establish_connection().await.map_err(service::db_err)?;
    let org_id = app_org(&db, app_id).await?;
    let app = service::load_app_in_org(&db, org_id, app_id).await?;
    let out = service::write_access(&db, &app, actor.id, &req).await?;

    // Into the ORG's append-only log, not just our own tracing. This is the one path
    // where Oxy staff reach into a tenant and change who may see the tenant's data —
    // both siblings already audit (`/admin/apps/{id}/publish` records `app.published`,
    // and the partner twin records `partner.app.access_changed`), and this being the
    // exception would leave the most sensitive of the three invisible to the org.
    audit::record_best_effort(
        &db,
        AuditEntry::for_request(&actor, "admin.app.access_changed")
            // `User`, matching the sibling `app.published` entry — the actor tier is
            // conveyed by the `admin.` action prefix, not by re-typing the actor.
            .org(org_id)
            .target(
                "app",
                app_id.to_string(),
                format!("{} ({})", app.name, app.slug),
            ),
    )
    .await;
    Ok(Json(out))
}

/// `GET /admin/apps/{app_id}/teams` — the owning org's teams, so the console can
/// offer the same grant picker the org sees. Keyed by APP rather than org so staff
/// never have to hold an org id, and so the tenant boundary is derived, not passed.
pub async fn list_app_org_teams(
    Path(app_id): Path<Uuid>,
) -> Result<Json<Vec<TeamDto>>, StatusCode> {
    let db = establish_connection().await.map_err(service::db_err)?;
    let org_id = app_org(&db, app_id).await?;
    Ok(Json(service::list_org_teams(&db, org_id).await?))
}

/// `GET /admin/apps/{app_id}/members` — the owning org's people, so the console can
/// grant an individual as well as a team. Same app-keyed shape as the teams route
/// above: the tenant is derived, never passed.
pub async fn list_app_org_members(
    Path(app_id): Path<Uuid>,
) -> Result<Json<Vec<OrgMemberOptionDto>>, StatusCode> {
    let db = establish_connection().await.map_err(service::db_err)?;
    let org_id = app_org(&db, app_id).await?;
    Ok(Json(service::list_org_member_options(&db, org_id).await?))
}
