//! `POST /api/admin/workspaces/{workspace_id}/transfer-org` — staff moving a
//! workspace under a different organization.
//!
//! Its own file because an org has one workspace: the handler grew the
//! one-per-org refusal and the coded-body helpers that go with it, and
//! [`super::workspaces`] was already past the file size limit.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use chrono::Utc;
use entity::{organizations, workspaces};
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_server_authz::staff::scope;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, Set};
use serde::Deserialize;
use uuid::Uuid;

use super::workspaces::internal;

#[derive(Deserialize)]
pub struct TransferOrgBody {
    pub new_org_id: Uuid,
}

/// Move a workspace under a different organization. Updates the workspace
/// row's `org_id` and bumps `updated_at`; **does not touch
/// `workspace_members`**.
///
/// Policy rationale (membership reconciliation is intentional non-action):
/// - `workspace_members` is keyed on (workspace_id, user_id). The FK is to
///   the workspace and the user, not to the org, so existing memberships
///   stay valid against the new org without any modification.
/// - If a member of the *old* org isn't a member of the *new* org, we
///   deliberately preserve their workspace access. Scrubbing those rows
///   would silently break setups where a workspace is being shared across
///   orgs intentionally (e.g. a consultant in the previous org continuing
///   work on the transferred workspace).
/// - Operators who *do* want to audit per-member access after a transfer
///   should pair this endpoint with the per-workspace member list
///   (`GET /admin/workspaces/{workspace_id}/detail`) and the per-user org
///   membership list (`GET /admin/users/{user_id}/org-memberships`) — both
///   already enumerate every membership row this handler is leaving
///   untouched.
///
/// **An org has one workspace**, so a target org that already has one is
/// `409 org_has_workspace`, the answer `register_workspace` gives a second
/// create. Checked here for that coded answer; the unique index
/// `uq_workspaces_org_id` is what holds against a race, and its violation gets
/// the same answer. A new org has no workspace until its repository is
/// connected, so it is always a valid target.
///
/// Only the workspace row moves. Its apps keep `apps.org_id`, and token grants
/// narrowed to it keep the old org: see the grain memo §7.2 before using this.
pub async fn transfer_org(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path(workspace_id): Path<Uuid>,
    Json(body): Json<TransferOrgBody>,
) -> Result<StatusCode, Coded> {
    let db = establish_connection()
        .await
        .map_err(|e| bare(internal(e)))?;
    // Both ends: fencing only the source would let a bounded grant move a workspace it
    // legitimately holds INTO an org it has no reach over.
    scope::deny_out_of_scope(&db, &actor, body.new_org_id)
        .await
        .map_err(bare)?;
    organizations::Entity::find_by_id(body.new_org_id)
        .one(&db)
        .await
        .map_err(|e| bare(internal(e)))?
        .ok_or_else(|| bare(StatusCode::NOT_FOUND))?;

    let ws = workspaces::Entity::find_by_id(workspace_id)
        .one(&db)
        .await
        .map_err(|e| bare(internal(e)))?
        .ok_or_else(|| bare(StatusCode::NOT_FOUND))?;
    // Scope: keyed by the workspace's OWNING org, resolved above — a workspace is not
    // itself scopeable, so the fence has to read through it. See `admin::scope`.
    scope::deny_out_of_scope_opt(&db, &actor, ws.org_id)
        .await
        .map_err(bare)?;
    if holds_another_workspace(&db, body.new_org_id, workspace_id)
        .await
        .map_err(|e| bare(internal(e)))?
    {
        return Err(org_has_workspace());
    }

    let mut active: workspaces::ActiveModel = ws.into();
    active.org_id = Set(Some(body.new_org_id));
    active.updated_at = Set(Utc::now().fixed_offset());
    active.update(&db).await.map_err(|e| match e.sql_err() {
        Some(sea_orm::SqlErr::UniqueConstraintViolation(_)) => org_has_workspace(),
        _ => bare(internal(e)),
    })?;
    tracing::info!(
        admin_email = %actor.label(),
        target_id = %workspace_id,
        new_org_id = %body.new_org_id,
        action = "transfer_workspace_org",
        "admin tenant action"
    );
    Ok(StatusCode::NO_CONTENT)
}

/// A refusal with a code in its body, for the routes that give one.
type Coded = (StatusCode, Json<serde_json::Value>);

/// A status with an empty body, for the refusals that need no code.
fn bare(status: StatusCode) -> Coded {
    (status, Json(serde_json::json!({})))
}

fn org_has_workspace() -> Coded {
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({
            "error": "org_has_workspace",
            "message": "The target organization already has its workspace, and an organization has one. Move this workspace into a new organization instead.",
        })),
    )
}

/// Whether `org_id` has a workspace other than `workspace_id`. Moving a
/// workspace into the org it is already in is not a second one.
async fn holds_another_workspace(
    db: &sea_orm::DatabaseConnection,
    org_id: Uuid,
    workspace_id: Uuid,
) -> Result<bool, sea_orm::DbErr> {
    let others = workspaces::Entity::find()
        .filter(workspaces::Column::OrgId.eq(org_id))
        .filter(workspaces::Column::Id.ne(workspace_id))
        .count(db)
        .await?;
    Ok(others > 0)
}
