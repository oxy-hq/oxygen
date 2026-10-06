//! `/api/{workspace_id}/oxy-access` — the **Oxy-staff lockdown** switch.
//!
//! Model (inverted 2026-07-14; migration `m20260714_000002`):
//!   * **Default: Oxy staff CAN access** this workspace's custom apps. No row,
//!     no friction — support works out of the box.
//!   * A row in `workspace_oxy_lockdown` means the org has **locked Oxy staff
//!     out**. While locked, no `app_admins` member can reach the workspace's apps.
//!
//! **Tenant-sovereign.** Locking/unlocking requires a *real* org owner/admin. The
//! previous consent toggle was guarded on the workspace `Owner` role — but
//! `workspace_context::resolve_effective_role` synthesizes an Owner membership for
//! any Global Owner / Global Admin, so **Oxy staff could grant themselves the very
//! access the toggle existed to gate**. The guard here rejects that synthetic
//! override (`WorkspaceGlobalOverride`), so an operator cannot unlock themselves.
//!
//! See [`crate::server::api::custom_apps_auth`] for how the lockdown is
//! consulted on the request path.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use entity::prelude::{WorkspaceOxyLockdown, Workspaces};
use entity::workspace_members::WorkspaceRole;
use entity::workspace_oxy_lockdown;
use oxy::database::client::establish_connection;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::server::api::custom_apps_auth::invalidate_access_cache;
use crate::server::api::middlewares::workspace_context::{
    EffectiveWorkspaceRole, WorkspaceGlobalOverride,
};

/// Status payload. `locked` is always present; the audit fields are only
/// meaningful when `locked` is true. `can_manage` tells the UI whether THIS
/// caller may flip the switch (a real org officer) — an Oxy operator sees the
/// state but cannot change it.
#[derive(Serialize)]
pub struct OxyLockdownStatus {
    pub locked: bool,
    pub locked_by: Option<Uuid>,
    pub locked_at: Option<String>,
    pub can_manage: bool,
}

impl OxyLockdownStatus {
    fn unlocked(can_manage: bool) -> Self {
        Self {
            locked: false,
            locked_by: None,
            locked_at: None,
            can_manage,
        }
    }

    fn from_row(row: workspace_oxy_lockdown::Model, can_manage: bool) -> Self {
        Self {
            locked: true,
            locked_by: row.locked_by,
            locked_at: Some(row.created_at.to_rfc3339()),
            can_manage,
        }
    }
}

#[derive(Deserialize)]
pub struct WorkspaceIdPath {
    pub workspace_id: Uuid,
}

/// Only a REAL org owner/admin may change the lockdown. Rejects:
///   * members/viewers (403), and
///   * the synthesized global-operator Owner — the whole point: Oxy staff must
///     not be able to unlock themselves out of a customer's lockdown.
fn require_real_org_officer(
    role: WorkspaceRole,
    WorkspaceGlobalOverride(is_override): WorkspaceGlobalOverride,
) -> Result<(), StatusCode> {
    if is_override {
        tracing::warn!(
            "oxy-access lockdown: rejected a global-operator override — only a real org officer may change it"
        );
        return Err(StatusCode::FORBIDDEN);
    }
    if role < WorkspaceRole::Admin {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

/// Whether this caller may flip the switch (drives `can_manage` in the payload).
fn can_manage(role: WorkspaceRole, ovr: WorkspaceGlobalOverride) -> bool {
    require_real_org_officer(role, ovr).is_ok()
}

pub async fn get_oxy_access(
    EffectiveWorkspaceRole(role): EffectiveWorkspaceRole,
    ovr: WorkspaceGlobalOverride,
    Path(WorkspaceIdPath { workspace_id }): Path<WorkspaceIdPath>,
) -> Result<Json<OxyLockdownStatus>, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("get_oxy_access DB connect failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let row = WorkspaceOxyLockdown::find()
        .filter(workspace_oxy_lockdown::Column::WorkspaceId.eq(workspace_id))
        .one(&db)
        .await
        .map_err(|e| {
            tracing::error!("get_oxy_access query failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let manage = can_manage(role, ovr);
    Ok(Json(match row {
        Some(r) => OxyLockdownStatus::from_row(r, manage),
        None => OxyLockdownStatus::unlocked(manage),
    }))
}

/// `POST` — lock Oxy staff OUT of this workspace.
/// The Oxy-access switch is the WorkspaceAdminStrict ring: a REAL workspace officer,
/// never the global-operator override (staff must not be able to unlock themselves).
/// `require_real_org_officer` already states that rule in one place; this routes the
/// same verdict through the model so the rule is declared there too. `existing &&
/// unified` — the model can only tighten. The workspace's org is looked up so the
/// org->workspace hierarchy resolves; without one (single-workspace local mode) the
/// org model doesn't apply and the legacy verdict stands.
async fn enforce_oxy_access(
    db: &sea_orm::DatabaseConnection,
    caller: &crate::server::authz::Caller,
    workspace_id: Uuid,
    legacy: bool,
) -> bool {
    let org_id = Workspaces::find_by_id(workspace_id)
        .one(db)
        .await
        .ok()
        .flatten()
        .and_then(|w| w.org_id);
    match org_id {
        Some(org_id) => {
            crate::server::authz::enforce_for(
                db,
                caller,
                "workspace.oxy_access",
                crate::server::authz::Action::WorkspaceOxyAccess,
                crate::server::authz::Resource::workspace(workspace_id, org_id),
                legacy,
            )
            .await
        }
        None => legacy,
    }
}

pub async fn lock_oxy_access(
    EffectiveWorkspaceRole(role): EffectiveWorkspaceRole,
    ovr: WorkspaceGlobalOverride,
    Path(WorkspaceIdPath { workspace_id }): Path<WorkspaceIdPath>,
    actor: oxy_app_core::audit::RequestActor,
) -> Result<Json<OxyLockdownStatus>, StatusCode> {
    let legacy = require_real_org_officer(role, ovr).is_ok();
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("lock_oxy_access DB connect failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    if !enforce_oxy_access(
        &db,
        &crate::server::authz::caller_of(&actor),
        workspace_id,
        legacy,
    )
    .await
    {
        return Err(StatusCode::FORBIDDEN);
    }

    if let Some(existing) = WorkspaceOxyLockdown::find()
        .filter(workspace_oxy_lockdown::Column::WorkspaceId.eq(workspace_id))
        .one(&db)
        .await
        .map_err(|e| {
            tracing::error!("lock_oxy_access existence check failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
    {
        return Ok(Json(OxyLockdownStatus::from_row(existing, true)));
    }

    let model = workspace_oxy_lockdown::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        workspace_id: ActiveValue::Set(workspace_id),
        locked_by: ActiveValue::Set(Some(actor.id)),
        created_at: ActiveValue::NotSet,
    }
    .insert(&db)
    .await
    .map_err(|e| {
        tracing::error!("lock_oxy_access insert failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Staff lose access on the next request, not at TTL.
    invalidate_access_cache();
    tracing::info!(
        %workspace_id, actor = %actor.label(),
        "oxy-access: workspace LOCKED Oxy staff out"
    );
    Ok(Json(OxyLockdownStatus::from_row(model, true)))
}

/// `DELETE` — remove the lockdown (restore the default: staff may access).
pub async fn unlock_oxy_access(
    EffectiveWorkspaceRole(role): EffectiveWorkspaceRole,
    ovr: WorkspaceGlobalOverride,
    Path(WorkspaceIdPath { workspace_id }): Path<WorkspaceIdPath>,
    actor: oxy_app_core::audit::RequestActor,
) -> Result<StatusCode, StatusCode> {
    let legacy = require_real_org_officer(role, ovr).is_ok();
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("unlock_oxy_access DB connect failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    if !enforce_oxy_access(
        &db,
        &crate::server::authz::caller_of(&actor),
        workspace_id,
        legacy,
    )
    .await
    {
        return Err(StatusCode::FORBIDDEN);
    }

    WorkspaceOxyLockdown::delete_many()
        .filter(workspace_oxy_lockdown::Column::WorkspaceId.eq(workspace_id))
        .exec(&db)
        .await
        .map_err(|e| {
            tracing::error!("unlock_oxy_access delete failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    invalidate_access_cache();
    tracing::info!(
        %workspace_id, actor = %actor.label(),
        "oxy-access: workspace lockdown lifted"
    );
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE regression guard. An Oxy operator reaches a tenant workspace with a
    /// *synthesized* Owner role; they must not be able to lift the customer's
    /// lockdown. This is the hole the old consent toggle had.
    #[test]
    fn global_operator_override_cannot_change_the_lockdown() {
        let ovr = WorkspaceGlobalOverride(true);
        // Even with a synthetic Owner role:
        assert_eq!(
            require_real_org_officer(WorkspaceRole::Owner, ovr),
            Err(StatusCode::FORBIDDEN)
        );
        assert!(!can_manage(WorkspaceRole::Owner, ovr));
    }

    #[test]
    fn real_org_officer_may_change_the_lockdown() {
        let real = WorkspaceGlobalOverride(false);
        assert_eq!(require_real_org_officer(WorkspaceRole::Owner, real), Ok(()));
        assert_eq!(require_real_org_officer(WorkspaceRole::Admin, real), Ok(()));
    }

    #[test]
    fn members_and_viewers_may_not() {
        let real = WorkspaceGlobalOverride(false);
        assert_eq!(
            require_real_org_officer(WorkspaceRole::Member, real),
            Err(StatusCode::FORBIDDEN)
        );
        assert_eq!(
            require_real_org_officer(WorkspaceRole::Viewer, real),
            Err(StatusCode::FORBIDDEN)
        );
    }
}
