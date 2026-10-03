//! Workspace health for a partner's managed clients — the scoped read of the
//! admin cross-tenant health rollup, filtered to the workspaces in the orgs this
//! partner manages. Read-only; the same persisted sweep state, same worst-first
//! ordering, just its subtree.

use axum::Json;
use axum::http::StatusCode;
use entity::prelude::Workspaces;
use entity::workspaces;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::{db, internal};
use crate::partner_console::partner_context::PartnerActor;
use oxy_app::server::api::admin::WorkspaceHealthRow;
use oxy_app::server::api::admin::health_rollup;
use oxy_server_authz::partner_authz::PartnerCapability;

/// `GET /partners/{id}/health` — health across the partner's managed clients'
/// workspaces, worst-first. Gated on `manage_apps` (the app/data plane the health
/// signal is about); the org set is the partner's own, so no per-org scope probe.
pub async fn list_health(actor: PartnerActor) -> Result<Json<Vec<WorkspaceHealthRow>>, StatusCode> {
    actor.require(PartnerCapability::ManageApps)?;
    let db = db().await?;

    // Every operator reaches all the partner's managed clients.
    let managed = &actor.0.org_ids;
    if managed.is_empty() {
        return Ok(Json(Vec::new()));
    }
    let workspace_ids: Vec<Uuid> = Workspaces::find()
        .filter(workspaces::Column::OrgId.is_in(managed.clone()))
        .all(&db)
        .await
        .map_err(internal("load workspaces"))?
        .into_iter()
        .map(|w| w.id)
        .collect();

    let rows = health_rollup(&db, Some(&workspace_ids))
        .await
        .map_err(internal("health rollup"))?;
    Ok(Json(rows))
}
