//! `/orgs/{org_id}/apps/{app_id}/access` — the ORG's own view of who may
//! open one of its custom apps.
//!
//! This is the control plane the enforcement engine has been waiting for.
//! `apps.visibility` and `app_members` shipped with `Ring::AppAccess` in m20260722,
//! but nothing in the product could write either — every `ActiveModel` left
//! `visibility` `NotSet`. These handlers are the trigger on that gun.
//!
//! Behavior lives in [`super::service`]; this file is the org surface's gate plus
//! two thin handlers. The Oxy admin panel and the partner console reach the same
//! service through their own gates — see that module's header for why the three
//! cannot share one route.

use axum::{Json, extract::Path, http::StatusCode};
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_server_authz::role_guards::OrgAdmin;
use uuid::Uuid;

use super::audit;
use super::dto::{AppAccessDto, AppAccessSummaryDto, SetAppAccessRequest};
use super::handlers::enforce_team_manage;
use super::service;

/// `GET /orgs/{org_id}/apps` — every app in the org with its visibility
/// and grant count, for the org's "who can open what" settings list.
///
/// Deliberately NOT the launcher's list: that one is filtered to what the VIEWER
/// may open, which is the opposite of what an admin managing access needs to see.
pub async fn list_org_apps(
    OrgAdmin(ctx): OrgAdmin,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
) -> Result<Json<Vec<AppAccessSummaryDto>>, StatusCode> {
    let db = establish_connection().await.map_err(service::db_err)?;
    enforce_team_manage(
        &db,
        actor.id,
        actor.email.as_deref().unwrap_or(""),
        ctx.org.id,
    )
    .await?;
    Ok(Json(
        service::list_org_apps_with_access(&db, ctx.org.id).await?,
    ))
}

/// `GET /orgs/{org_id}/apps/{app_id}/access`
pub async fn get_app_access(
    OrgAdmin(ctx): OrgAdmin,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((_org_id, app_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<AppAccessDto>, StatusCode> {
    let db = establish_connection().await.map_err(service::db_err)?;
    enforce_team_manage(
        &db,
        actor.id,
        actor.email.as_deref().unwrap_or(""),
        ctx.org.id,
    )
    .await?;
    let app = service::load_app_in_org(&db, ctx.org.id, app_id).await?;
    Ok(Json(service::read_access(&db, &app).await?))
}

/// `PUT /orgs/{org_id}/apps/{app_id}/access`
pub async fn set_app_access(
    OrgAdmin(ctx): OrgAdmin,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((_org_id, app_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<SetAppAccessRequest>,
) -> Result<Json<AppAccessDto>, StatusCode> {
    let db = establish_connection().await.map_err(service::db_err)?;
    enforce_team_manage(
        &db,
        actor.id,
        actor.email.as_deref().unwrap_or(""),
        ctx.org.id,
    )
    .await?;
    let app = service::load_app_in_org(&db, ctx.org.id, app_id).await?;
    let label = format!("{} ({})", app.name, app.slug);
    let out = service::write_access(&db, &app, actor.id, &req).await?;

    // Both other doors onto `write_access` record; this one is the door the
    // launcher's Access button opens, so it is the likeliest of the three — and it
    // is reachable by Oxy staff, whom `OrgAdmin` admits through a synthesized Owner
    // membership. `audit::record` reads that off the context and files the write
    // under the same `admin.` name the admin console uses.
    audit::record(
        &db,
        &ctx,
        &actor,
        audit::APP_ACCESS_CHANGED,
        ("app", app_id, label),
    )
    .await;
    Ok(Json(out))
}
