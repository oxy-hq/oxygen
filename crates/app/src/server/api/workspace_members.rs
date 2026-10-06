use std::str::FromStr;

use axum::extract::{Json, Path};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use entity::org_members::OrgRole;
use entity::prelude::{OrgMembers, Users, WorkspaceMembers};
use entity::workspace_members::WorkspaceRole;
use oxy::database::client::establish_connection;

use crate::server::authz;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::server::api::middlewares::workspace_context::OrgMembershipExtractor;
use crate::server::router::WorkspaceExtractor;

// Request / Response types

#[derive(Serialize)]
pub struct WorkspaceMemberResponse {
    pub user_id: Uuid,
    pub email: String,
    pub name: String,
    pub org_role: String,
    pub workspace_role: String,
    pub is_override: bool,
}

#[derive(Deserialize)]
pub struct SetRoleRequest {
    pub role: String,
}

#[derive(serde::Deserialize)]
pub struct WorkspaceMemberPath {
    pub workspace_id: Uuid,
    pub user_id: Uuid,
}

// Helpers

pub(crate) fn map_org_role_to_workspace(org_role: &OrgRole) -> WorkspaceRole {
    match org_role {
        OrgRole::Owner => WorkspaceRole::Owner,
        OrgRole::Admin => WorkspaceRole::Admin,
        OrgRole::Member => WorkspaceRole::Member,
    }
}

/// Validate that a role override is allowed given the caller's and target's org roles.
///
/// Rules:
/// - Caller must be at least Admin (checked before calling this).
/// - Caller cannot set overrides on users with equal or higher org role.
/// - Only Owners can grant workspace Owner role.
pub(crate) fn validate_role_override(
    caller: &entity::org_members::Model,
    target: &entity::org_members::Model,
    requested_role: &WorkspaceRole,
) -> Result<(), StatusCode> {
    // Caller cannot modify users at or above their own org role.
    if target.role >= caller.role {
        return Err(StatusCode::FORBIDDEN);
    }

    // Only Org Owners can grant the workspace Owner role. Same invariant as granting an
    // org Owner — "only an Owner mints an Owner" — so it shares member_authz's rule
    // rather than restating it against a different role enum.
    oxy_app_core::member_authz::authorize_owner_grant(
        &caller.role,
        *requested_role == WorkspaceRole::Owner,
    )?;

    Ok(())
}

// Endpoints

/// List all org members with their effective workspace role.
pub async fn list_workspace_members(
    WorkspaceExtractor(workspace): WorkspaceExtractor,
) -> Result<Json<Vec<WorkspaceMemberResponse>>, StatusCode> {
    // Legacy workspaces (no org) don't have the concept of workspace members.
    let Some(org_id) = workspace.org_id else {
        return Ok(Json(vec![]));
    };

    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    use entity::org_members::Column as OmCol;
    let org_members = OrgMembers::find()
        .filter(OmCol::OrgId.eq(org_id))
        .all(&db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    use entity::workspace_members::Column as WmCol;
    let ws_overrides = WorkspaceMembers::find()
        .filter(WmCol::WorkspaceId.eq(workspace.id))
        .all(&db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Collect user IDs to fetch user details
    let user_ids: Vec<Uuid> = org_members.iter().map(|m| m.user_id).collect();

    use entity::users::Column as UserCol;
    let users = Users::find()
        .filter(UserCol::Id.is_in(user_ids))
        .all(&db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let user_map: std::collections::HashMap<Uuid, &entity::users::Model> =
        users.iter().map(|u| (u.id, u)).collect();

    let override_map: std::collections::HashMap<Uuid, &entity::workspace_members::Model> =
        ws_overrides.iter().map(|o| (o.user_id, o)).collect();

    let members: Vec<WorkspaceMemberResponse> = org_members
        .iter()
        .filter_map(|om| {
            let user = user_map.get(&om.user_id)?;
            let org_derived = map_org_role_to_workspace(&om.role);
            let (workspace_role, is_override) = match override_map.get(&om.user_id) {
                Some(ws_member) => {
                    // Consistent with middleware: override can only elevate.
                    (std::cmp::max(org_derived, ws_member.role.clone()), true)
                }
                None => (org_derived, false),
            };
            Some(WorkspaceMemberResponse {
                user_id: om.user_id,
                email: user.label().to_string(),
                name: user.name.clone(),
                org_role: om.role.as_str().to_string(),
                workspace_role: workspace_role.as_str().to_string(),
                is_override,
            })
        })
        .collect();

    Ok(Json(members))
}

pub async fn set_workspace_role_override(
    WorkspaceExtractor(workspace): WorkspaceExtractor,
    OrgMembershipExtractor(org_membership): OrgMembershipExtractor,
    actor: oxy_app_core::audit::RequestActor,
    Path(WorkspaceMemberPath {
        workspace_id: _,
        user_id,
    }): Path<WorkspaceMemberPath>,
    Json(body): Json<SetRoleRequest>,
) -> Result<Json<serde_json::Value>, Response> {
    let role = WorkspaceRole::from_str(&body.role)
        .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY.into_response())?;

    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;

    // Managing an org's members is the OrgAdmin ring. Enforce it through the policy
    // rather than re-deriving `Owner|Admin` inline — an inline copy is how a call site
    // drifts from the model it thinks it implements.
    let legacy = matches!(org_membership.role, OrgRole::Owner | OrgRole::Admin);
    let allowed = authz::enforce_for(
        &db,
        &crate::server::authz::caller_of(&actor),
        "workspace_members.set_role",
        authz::Action::MemberSetRole,
        authz::Resource::org(org_membership.org_id),
        legacy,
    )
    .await;
    if !allowed {
        return Err(StatusCode::FORBIDDEN.into_response());
    }

    // Fetch the target user's org membership to check role hierarchy.
    let target_membership = if let Some(org_id) = workspace.org_id {
        use entity::org_members::Column as OmCol;
        entity::prelude::OrgMembers::find()
            .filter(OmCol::OrgId.eq(org_id))
            .filter(OmCol::UserId.eq(user_id))
            .one(&db)
            .await
            .map_err(|e| {
                tracing::error!("Failed to check target user org membership: {e}");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            })?
    } else {
        None
    };

    let Some(target_membership) = target_membership else {
        return Err(StatusCode::NOT_FOUND.into_response());
    };

    validate_role_override(&org_membership, &target_membership, &role)
        .map_err(|s| s.into_response())?;

    // Check if override already exists
    use entity::workspace_members::Column as WmCol;
    let existing = WorkspaceMembers::find()
        .filter(WmCol::WorkspaceId.eq(workspace.id))
        .filter(WmCol::UserId.eq(user_id))
        .one(&db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;

    if let Some(existing_member) = existing {
        // Update existing override
        let mut active: entity::workspace_members::ActiveModel = existing_member.into();
        active.role = ActiveValue::Set(role);
        active.updated_at = ActiveValue::Set(Utc::now().into());
        active.update(&db).await.map_err(|e| {
            tracing::error!("Failed to update workspace member override: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        })?;
    } else {
        // Insert new override
        let new_member = entity::workspace_members::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            workspace_id: ActiveValue::Set(workspace.id),
            user_id: ActiveValue::Set(user_id),
            role: ActiveValue::Set(role),
            created_at: ActiveValue::Set(Utc::now().into()),
            updated_at: ActiveValue::Set(Utc::now().into()),
        };
        new_member.insert(&db).await.map_err(|e| {
            tracing::error!("Failed to insert workspace member override: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        })?;
    }

    // Effective workspace role just changed for this user; their cached
    // airhouse credential carries the OLD role. Revoke it so the next mint
    // produces a credential under the new role and the old one stops
    // authenticating immediately.
    if let Some(broker) = airhouse::token_broker() {
        broker.revoke_user_across_roles(workspace.id, user_id).await;
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// Remove a workspace role override (revert to org-derived role).
pub async fn remove_workspace_role_override(
    WorkspaceExtractor(workspace): WorkspaceExtractor,
    OrgMembershipExtractor(org_membership): OrgMembershipExtractor,
    actor: oxy_app_core::audit::RequestActor,
    Path(WorkspaceMemberPath {
        workspace_id: _,
        user_id,
    }): Path<WorkspaceMemberPath>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // OrgAdmin ring, via the policy — see `set_workspace_role_override`.
    let legacy = matches!(org_membership.role, OrgRole::Owner | OrgRole::Admin);
    let allowed = authz::enforce_for(
        &db,
        &crate::server::authz::caller_of(&actor),
        "workspace_members.remove_role",
        authz::Action::MemberRemove,
        authz::Resource::org(org_membership.org_id),
        legacy,
    )
    .await;
    if !allowed {
        return Err(StatusCode::FORBIDDEN);
    }

    // Verify caller outranks target in org hierarchy.
    if let Some(org_id) = workspace.org_id {
        use entity::org_members::Column as OmCol;
        let target = entity::prelude::OrgMembers::find()
            .filter(OmCol::OrgId.eq(org_id))
            .filter(OmCol::UserId.eq(user_id))
            .one(&db)
            .await
            .map_err(|e| {
                tracing::error!("Failed to check target user org membership: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        if let Some(t) = target
            && t.role >= org_membership.role
        {
            return Err(StatusCode::FORBIDDEN);
        }
    }

    use entity::workspace_members::Column as WmCol;
    let existing = WorkspaceMembers::find()
        .filter(WmCol::WorkspaceId.eq(workspace.id))
        .filter(WmCol::UserId.eq(user_id))
        .one(&db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let removed = if let Some(member) = existing {
        let active: entity::workspace_members::ActiveModel = member.into();
        active.delete(&db).await.map_err(|e| {
            tracing::error!("Failed to delete workspace member override: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        true
    } else {
        false
    };

    // The user's effective workspace role just reverted to the org-derived
    // value, which may not equal whatever the override was. Revoke any
    // cached airhouse credential so the role downgrade takes effect now,
    // not at the token's TTL.
    if removed && let Some(broker) = airhouse::token_broker() {
        broker.revoke_user_across_roles(workspace.id, user_id).await;
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

#[cfg(test)]
#[path = "workspace_members_tests.rs"]
mod workspace_members_tests;
