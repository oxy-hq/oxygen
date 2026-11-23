//! `GET /api/customer-apps/oxy-access` — the staff "what may we touch" list.
//!
//! **Inverted 2026-07-14.** This used to list only workspaces that had *granted*
//! Oxy access (a `workspace_oxy_access` row). Staff access is now the DEFAULT, and
//! a row in `workspace_oxy_lockdown` REVOKES it — so the useful list is *every*
//! workspace, each flagged with whether the org has locked us out.
//!
//! That keeps this the canonical "what may we touch" answer (`accessible` is the
//! single field to read) while also surfacing the workspaces we're locked out of,
//! which is exactly what an operator needs to see when an app won't open.
//!
//! Gated by the `/customer-apps` nest's `oxy_owner_or_app_admin_guard`.

use std::collections::HashMap;

use axum::Json;
use axum::http::StatusCode;
use entity::prelude::{Organizations, Users, WorkspaceOxyLockdown, Workspaces};
use entity::{organizations, users, workspace_oxy_lockdown, workspaces};
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Serialize;
use uuid::Uuid;

/// One workspace in the staff browser, flattened with its org and lockdown state.
#[derive(Debug, Serialize)]
pub struct OxyAccessRow {
    pub workspace_id: Uuid,
    pub workspace_name: String,
    pub org_id: Uuid,
    pub org_name: String,
    pub org_slug: String,
    /// The one field that matters: may Oxy staff touch this workspace's apps?
    pub accessible: bool,
    /// True when the org has locked Oxy staff out (`accessible == !locked`).
    pub locked: bool,
    /// Email of the org officer who locked us out, if still resolvable.
    pub locked_by_email: Option<String>,
    pub locked_at: Option<String>,
}

pub async fn list_grants(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
) -> Result<Json<Vec<OxyAccessRow>>, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("oxy-access list: DB connect failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Every workspace is a candidate now — access is the default.
    let mut query = Workspaces::find();
    // Scope exception #3 (see `app_scope_guard`): this route names no app at all — it
    // browses every org's workspaces to populate the "Add custom app" picker. A bounded
    // grant must see only its own orgs here, or the picker becomes a cross-tenant
    // directory listing.
    if let Some(orgs) = oxy_server_authz::staff::org_filter::scope_org_filter(&db, &user).await {
        query = query.filter(workspaces::Column::OrgId.is_in(orgs));
    }
    let all_ws = query.all(&db).await.map_err(db_err)?;
    if all_ws.is_empty() {
        return Ok(Json(vec![]));
    }

    let lockdowns = WorkspaceOxyLockdown::find()
        .all(&db)
        .await
        .map_err(db_err)?
        .into_iter()
        .map(|l| (l.workspace_id, l))
        .collect::<HashMap<_, _>>();

    let org_ids = all_ws.iter().filter_map(|w| w.org_id).collect::<Vec<_>>();
    let org_map = Organizations::find()
        .filter(organizations::Column::Id.is_in(org_ids))
        .all(&db)
        .await
        .map_err(db_err)?
        .into_iter()
        .map(|o| (o.id, o))
        .collect::<HashMap<_, _>>();

    let user_ids = lockdowns
        .values()
        .filter_map(|l| l.locked_by)
        .collect::<Vec<_>>();
    let email_map = Users::find()
        .filter(users::Column::Id.is_in(user_ids))
        .all(&db)
        .await
        .map_err(db_err)?
        .into_iter()
        .map(|u| (u.id, u.label().to_string()))
        .collect::<HashMap<_, _>>();

    let mut out: Vec<OxyAccessRow> = all_ws
        .into_iter()
        .filter_map(|ws| build_row(ws, &lockdowns, &org_map, &email_map))
        .collect();
    // Stable order for the browser: locked-out first (they're the exceptions an
    // operator is looking for), then org, then workspace.
    out.sort_by(|a, b| {
        b.locked
            .cmp(&a.locked)
            .then_with(|| a.org_name.cmp(&b.org_name))
            .then_with(|| a.workspace_name.cmp(&b.workspace_name))
    });
    Ok(Json(out))
}

/// Assemble one row, dropping workspaces with no resolvable org — those aren't
/// actionable in the browser.
fn build_row(
    ws: workspaces::Model,
    lockdowns: &HashMap<Uuid, workspace_oxy_lockdown::Model>,
    org_map: &HashMap<Uuid, organizations::Model>,
    email_map: &HashMap<Uuid, String>,
) -> Option<OxyAccessRow> {
    let org = org_map.get(&ws.org_id?)?;
    let lock = lockdowns.get(&ws.id);
    Some(OxyAccessRow {
        workspace_id: ws.id,
        workspace_name: ws.name.clone(),
        org_id: org.id,
        org_name: org.name.clone(),
        org_slug: org.slug.clone(),
        accessible: lock.is_none(),
        locked: lock.is_some(),
        locked_by_email: lock
            .and_then(|l| l.locked_by)
            .and_then(|id| email_map.get(&id).cloned()),
        locked_at: lock.map(|l| l.created_at.to_rfc3339()),
    })
}

fn db_err(e: sea_orm::DbErr) -> StatusCode {
    tracing::error!("oxy-access list query failed: {e}");
    StatusCode::INTERNAL_SERVER_ERROR
}
