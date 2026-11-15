//! `/api/{workspace_id}/org-subdomain` — **read-only** status of the org's
//! bare subdomain (`<org-slug>.<zone>`) for display in the customer's
//! settings. Owner-readable.
//!
//! Enable/disable is an **Oxy-staff** action in the admin panel
//! (`/api/admin/orgs/{org_id}/subdomain`) — a customer must not be able to
//! flip a live public surface on/off (it would break shared branded URLs and
//! `…/a/<slug>` app links). See
//! `internal-docs/org-subdomain-infra.md`.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use entity::org_subdomains;
use entity::prelude::{OrgSubdomains, Organizations, Workspaces};
use entity::workspace_members::WorkspaceRole;
use oxy::database::client::establish_connection;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use oxy_app::surface::EffectiveWorkspaceRole;
use oxy_app_core::org_host_dispatch;

#[derive(Serialize)]
pub struct OrgSubdomainStatus {
    pub enabled: bool,
    /// The org slug — this is the subdomain label.
    pub subdomain: String,
    /// Full URL `https://<slug>.<zone>/`; `None` when disabled or the zone
    /// isn't derivable (local dev).
    pub url: Option<String>,
    /// True when THIS workspace is the subdomain's default project.
    pub is_default_workspace: bool,
}

impl OrgSubdomainStatus {
    fn disabled() -> Self {
        Self {
            enabled: false,
            subdomain: String::new(),
            url: None,
            is_default_workspace: false,
        }
    }
}

#[derive(Deserialize)]
pub struct WorkspaceIdPath {
    pub workspace_id: Uuid,
}

fn require_owner(role: WorkspaceRole) -> Result<(), StatusCode> {
    if role >= WorkspaceRole::Owner {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

fn db_err<E: std::fmt::Display>(e: E) -> StatusCode {
    tracing::error!("org-subdomain: db error: {e}");
    StatusCode::INTERNAL_SERVER_ERROR
}

pub async fn get_org_subdomain(
    EffectiveWorkspaceRole(role): EffectiveWorkspaceRole,
    Path(WorkspaceIdPath { workspace_id }): Path<WorkspaceIdPath>,
) -> Result<Json<OrgSubdomainStatus>, StatusCode> {
    require_owner(role)?;
    let db = establish_connection().await.map_err(db_err)?;

    let ws = Workspaces::find_by_id(workspace_id)
        .one(&db)
        .await
        .map_err(db_err)?
        .ok_or(StatusCode::NOT_FOUND)?;
    // Org-less workspace (e.g. local mode) — the feature doesn't apply; report
    // disabled rather than erroring so the settings section renders cleanly.
    let Some(org_id) = ws.org_id else {
        return Ok(Json(OrgSubdomainStatus::disabled()));
    };
    let org = Organizations::find_by_id(org_id)
        .one(&db)
        .await
        .map_err(db_err)?
        .ok_or(StatusCode::NOT_FOUND)?;

    let row = OrgSubdomains::find()
        .filter(org_subdomains::Column::OrgId.eq(org.id))
        .one(&db)
        .await
        .map_err(db_err)?;

    let enabled = row.as_ref().map(|r| r.enabled).unwrap_or(false);
    let default_workspace_id = row.as_ref().and_then(|r| r.default_workspace_id);
    let url = if enabled {
        org_host_dispatch::org_subdomain_zone().map(|z| format!("https://{}.{z}/", org.slug))
    } else {
        None
    };
    Ok(Json(OrgSubdomainStatus {
        enabled,
        subdomain: org.slug,
        url,
        is_default_workspace: default_workspace_id == Some(workspace_id),
    }))
}
