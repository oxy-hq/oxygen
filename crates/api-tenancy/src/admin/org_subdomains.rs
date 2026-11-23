//! `/api/admin/orgs/{org_id}/subdomain` — Oxy-staff control of an org's bare
//! subdomain (`<org-slug>.<zone>`). Gated by the admin surface's outer guard
//! (OXY_OWNER or app_admins). The subdomain label IS the org slug — not
//! configurable. Customers can't toggle this (it serves a live public
//! surface); they see read-only status via `/api/{workspace_id}/org-subdomain`.
//! See `internal-docs/org-subdomain-infra.md`.

use axum::extract::Path;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;
use entity::workspaces::WorkspaceStatus;
use entity::{org_subdomains, organizations, workspaces};
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use oxy_app_core::AppState;
use oxy_app_core::org_host_dispatch;

pub(crate) fn router() -> Router<AppState> {
    Router::new().route(
        "/orgs/{org_id}/subdomain",
        get(get_subdomain).put(set_subdomain),
    )
}

#[derive(Serialize)]
pub struct WorkspaceOption {
    pub id: Uuid,
    pub name: String,
    pub status: String,
}

#[derive(Serialize)]
pub struct AdminOrgSubdomainResponse {
    pub enabled: bool,
    /// The org slug — this is the subdomain label.
    pub subdomain: String,
    /// `https://<slug>.<zone>/`; `None` when the zone isn't derivable.
    pub url: Option<String>,
    pub default_workspace_id: Option<Uuid>,
    /// The org's workspaces, for the default-project dropdown.
    pub workspaces: Vec<WorkspaceOption>,
    /// True when the slug collides with a reserved infra label (can't enable).
    pub reserved: bool,
}

#[derive(Deserialize)]
pub struct SetOrgSubdomainBody {
    pub enabled: bool,
    pub default_workspace_id: Option<Uuid>,
}

type ApiError = (StatusCode, String);

fn db_err<E: std::fmt::Display>(e: E) -> ApiError {
    tracing::error!("admin org-subdomain: db error: {e}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal error".to_string(),
    )
}

fn status_label(s: &WorkspaceStatus) -> String {
    match s {
        WorkspaceStatus::Ready => "ready",
        WorkspaceStatus::Cloning => "cloning",
        WorkspaceStatus::Failed => "failed",
        WorkspaceStatus::NotOxyProject => "not_oxy_project",
    }
    .to_string()
}

async fn load_org(
    db: &sea_orm::DatabaseConnection,
    org_id: Uuid,
) -> Result<organizations::Model, ApiError> {
    organizations::Entity::find_by_id(org_id)
        .one(db)
        .await
        .map_err(db_err)?
        .ok_or((StatusCode::NOT_FOUND, "org not found".to_string()))
}

async fn build_response(
    db: &sea_orm::DatabaseConnection,
    org: &organizations::Model,
) -> Result<AdminOrgSubdomainResponse, ApiError> {
    let row = org_subdomains::Entity::find()
        .filter(org_subdomains::Column::OrgId.eq(org.id))
        .one(db)
        .await
        .map_err(db_err)?;
    let workspaces = workspaces::Entity::find()
        .filter(workspaces::Column::OrgId.eq(org.id))
        .order_by_desc(workspaces::Column::CreatedAt)
        .all(db)
        .await
        .map_err(db_err)?
        .into_iter()
        .map(|w| WorkspaceOption {
            id: w.id,
            name: w.name,
            status: status_label(&w.status),
        })
        .collect();
    Ok(AdminOrgSubdomainResponse {
        enabled: row.as_ref().map(|r| r.enabled).unwrap_or(false),
        subdomain: org.slug.clone(),
        url: org_host_dispatch::org_subdomain_zone().map(|z| format!("https://{}.{z}/", org.slug)),
        default_workspace_id: row.and_then(|r| r.default_workspace_id),
        workspaces,
        reserved: org_host_dispatch::is_reserved_label(&org.slug),
    })
}

/// Bridge the shared fence into this module's `ApiError`.
///
/// Named for what it calls, not for what it is: the coverage test looks for
/// `deny_out_of_scope` in a handler body, and a wrapper called `fence` made the two
/// fenced handlers here read as unfenced.
async fn deny_out_of_scope(
    db: &sea_orm::DatabaseConnection,
    actor: &oxy_auth::types::AuthenticatedUser,
    org_id: Uuid,
) -> Result<(), ApiError> {
    oxy_server_authz::staff::scope::deny_out_of_scope(db, actor, org_id)
        .await
        // 404 stays 404: the message is generic on purpose, so an out-of-scope caller
        // learns nothing the status doesn't already say.
        .map_err(|status| (status, "not found".to_string()))
}

pub async fn get_subdomain(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path(org_id): Path<Uuid>,
) -> Result<Json<AdminOrgSubdomainResponse>, ApiError> {
    let db = establish_connection().await.map_err(db_err)?;
    deny_out_of_scope(&db, &actor, org_id).await?;
    let org = load_org(&db, org_id).await?;
    Ok(Json(build_response(&db, &org).await?))
}

pub async fn set_subdomain(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path(org_id): Path<Uuid>,
    Json(body): Json<SetOrgSubdomainBody>,
) -> Result<Json<AdminOrgSubdomainResponse>, ApiError> {
    let db = establish_connection().await.map_err(db_err)?;
    // Scope. This router is merged on the SAME capability as `orgs_admin`
    // (`cap(Action::PlatformOrgs)`) and owns an `/orgs/{org_id}` write, but was never
    // part of the sweep that fenced its sibling — a second router on one capability,
    // which is round 12's shape stated a level up. Unfenced, a grant bounded to one
    // tenant could disable another's `<slug>.oxygen-hq.com`, and
    // `org_host_dispatch::invalidate_cache()` makes it take effect at once: a logo is
    // defacement, this is the tenant's production entry point.
    deny_out_of_scope(&db, &actor, org_id).await?;
    let org = load_org(&db, org_id).await?;

    if body.enabled && org_host_dispatch::is_reserved_label(&org.slug) {
        return Err((
            StatusCode::CONFLICT,
            format!("org slug '{}' is a reserved label", org.slug),
        ));
    }
    if let Some(ws_id) = body.default_workspace_id {
        let belongs = workspaces::Entity::find_by_id(ws_id)
            .one(&db)
            .await
            .map_err(db_err)?
            .map(|w| w.org_id == Some(org_id))
            .unwrap_or(false);
        if !belongs {
            return Err((
                StatusCode::BAD_REQUEST,
                "default project is not in this org".to_string(),
            ));
        }
    }

    let now = Utc::now();
    let existing = org_subdomains::Entity::find()
        .filter(org_subdomains::Column::OrgId.eq(org.id))
        .one(&db)
        .await
        .map_err(db_err)?;
    match existing {
        Some(e) => {
            let mut am: org_subdomains::ActiveModel = e.into();
            am.enabled = Set(body.enabled);
            am.default_workspace_id = Set(body.default_workspace_id);
            am.updated_at = Set(now.into());
            am.update(&db).await.map_err(db_err)?;
        }
        None => {
            org_subdomains::ActiveModel {
                id: Set(Uuid::new_v4()),
                org_id: Set(org.id),
                default_workspace_id: Set(body.default_workspace_id),
                enabled: Set(body.enabled),
                created_by: Set(Some(actor.id)),
                created_at: Set(now.into()),
                updated_at: Set(now.into()),
            }
            .insert(&db)
            .await
            .map_err(db_err)?;
        }
    }

    org_host_dispatch::invalidate_cache();
    Ok(Json(build_response(&db, &org).await?))
}
