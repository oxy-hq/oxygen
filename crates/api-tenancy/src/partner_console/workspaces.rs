//! Workspaces inside a partner-managed client org — the read side of the admin
//! workspaces surface, scoped to one client the partner manages.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use entity::prelude::Workspaces;
use entity::workspaces;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde::Serialize;
use uuid::Uuid;

use super::{db, internal, require_org_scope};
use crate::partner_console::partner_context::PartnerActor;
use oxy_server_authz::partner_authz::PartnerCapability;

#[derive(Serialize)]
pub struct ClientWorkspaceDto {
    pub id: Uuid,
    pub name: String,
    /// `ready` / `preparing` / `error` — lower-cased for the UI.
    pub status: String,
    /// Whether a compiled revision is pinned. A workspace with none opens empty.
    pub has_revision: bool,
    pub last_opened_at: Option<String>,
    pub updated_at: String,
    pub error: Option<String>,
}

/// `GET /partners/{id}/orgs/{org_id}/workspaces` — the workspaces in one client.
///
/// Gated on `manage_apps` (the app/data plane) and on the org being in the
/// partner's managed set (`require_org_scope`, which 404s an org outside it so the
/// partner can't probe for tenants it doesn't manage).
pub async fn list_org_workspaces(
    PartnerActor(scope): PartnerActor,
    Path((_partner_org_id, org_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<ClientWorkspaceDto>>, StatusCode> {
    let db = db().await?;
    require_org_scope(&db, &scope, org_id, PartnerCapability::ManageApps).await?;

    let rows = Workspaces::find()
        .filter(workspaces::Column::OrgId.eq(org_id))
        .order_by_desc(workspaces::Column::UpdatedAt)
        .all(&db)
        .await
        .map_err(internal("load workspaces"))?;

    let out = rows
        .into_iter()
        .map(|w| ClientWorkspaceDto {
            id: w.id,
            name: w.name,
            status: format!("{:?}", w.status).to_lowercase(),
            has_revision: w.current_revision_id.is_some(),
            last_opened_at: w.last_opened_at.map(|t| t.to_rfc3339()),
            updated_at: w.updated_at.to_rfc3339(),
            error: w.error,
        })
        .collect();
    Ok(Json(out))
}
