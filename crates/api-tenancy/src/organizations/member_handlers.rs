use std::str::FromStr;

use axum::extract::{Json, Path};
use axum::http::StatusCode;
use chrono::Utc;
use entity::org_members;
use entity::org_members::OrgRole;
use entity::prelude::*;
use entity::workspaces;
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter,
    QuerySelect, TransactionTrait,
};
use uuid::Uuid;

use oxy_app::surface::OrgContextExtractor;
use oxy_app::surface::role_guards::OrgAdmin;
use oxy_app_core::audit;
use oxy_app_core::member_authz;

use super::dto::*;

// Members management

/// GET /orgs/:org_id/members
pub async fn list_members(
    OrgContextExtractor(ctx): OrgContextExtractor,
) -> Result<Json<Vec<MemberResponse>>, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("DB connection error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let members = OrgMembers::find()
        .filter(org_members::Column::OrgId.eq(ctx.org.id))
        .find_also_related(Users)
        .all(&db)
        .await
        .map_err(|e| {
            tracing::error!("Failed to query members: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let responses: Vec<MemberResponse> = members
        .into_iter()
        .filter_map(|(member, user)| {
            let user = user?;
            Some(MemberResponse {
                id: member.id,
                user_id: member.user_id,
                email: user.label().to_string(),
                name: user.name,
                role: member.role.as_str().to_string(),
                created_at: member.created_at.to_rfc3339(),
            })
        })
        .collect();

    Ok(Json(responses))
}

/// PATCH /orgs/:org_id/members/:user_id
pub async fn update_member_role(
    OrgAdmin(ctx): OrgAdmin,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((_org_id, target_user_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<UpdateRoleRequest>,
) -> Result<Json<MemberResponse>, StatusCode> {
    // Validate the new role, then apply the pre-load authority rules
    // (owner-can't-self-demote + only-owner-grants-owner). See member_authz.
    let new_role = OrgRole::from_str(&req.role).map_err(|_| StatusCode::BAD_REQUEST)?;
    let is_self = target_user_id == ctx.membership.user_id;
    member_authz::authorize_role_change_intent(&ctx.membership.role, is_self, &new_role)?;

    let db = establish_connection().await.map_err(|e| {
        tracing::error!("DB connection error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Transaction with row-level locks on the org's owner rows prevents a
    // TOCTOU where two concurrent demotions both see owner_count > 1 and
    // drop the last owner.
    let txn = db.begin().await.map_err(|e| {
        tracing::error!("Failed to start transaction: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let target = OrgMembers::find()
        .filter(org_members::Column::OrgId.eq(ctx.org.id))
        .filter(org_members::Column::UserId.eq(target_user_id))
        .lock_exclusive()
        .one(&txn)
        .await
        .map_err(|e| {
            tracing::error!("Failed to query target member: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    // Only Owners can demote other Admins or Owners.
    member_authz::authorize_target_modification(&ctx.membership.role, &target.role)?;
    let previous_role = target.role.clone();

    // Cannot change the last owner's role. The lock_exclusive() above holds
    // an exclusive lock on the target row through commit; locking all other
    // owners here prevents concurrent owner-removing transactions from racing.
    if target.role == OrgRole::Owner && new_role != OrgRole::Owner {
        let owner_count = OrgMembers::find()
            .filter(org_members::Column::OrgId.eq(ctx.org.id))
            .filter(org_members::Column::Role.eq(OrgRole::Owner))
            .lock_exclusive()
            .count(&txn)
            .await
            .map_err(|e| {
                tracing::error!("Failed to count owners: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        if owner_count <= 1 {
            return Err(StatusCode::BAD_REQUEST);
        }
    }

    let mut active: org_members::ActiveModel = target.into();
    active.role = ActiveValue::Set(new_role.clone());
    active.updated_at = ActiveValue::Set(Utc::now().fixed_offset());
    let updated = active.update(&txn).await.map_err(|e| {
        tracing::error!("Failed to update member role: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let user = Users::find_by_id(target_user_id)
        .one(&txn)
        .await
        .map_err(|e| {
            tracing::error!("Failed to query user: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    // "Who changed this person's role" is exactly what an audit log is for, and it
    // was recorded ONLY when a partner did it — the org's own admins and Oxy staff
    // were invisible. An audit trail blind to its most privileged actors is worse
    // than none: it implies coverage it doesn't have.
    //
    // The actor is the REAL user, never the synthetic Owner that `org_context`
    // injects for staff — otherwise the log would launder an override into a
    // legitimate membership. `is_global_override` records that it happened through
    // one.
    audit::record_in_txn(
        &txn,
        audit::AuditEntry::new(actor.label().to_string(), "org.member.role_updated")
            .actor(actor.id, audit::ActorType::User)
            .org(ctx.org.id)
            .target("user", target_user_id.to_string(), user.label().to_string())
            .change(
                serde_json::json!({ "role": previous_role.as_str() }),
                serde_json::json!({ "role": new_role.as_str() }),
            )
            .metadata(serde_json::json!({ "via_global_override": ctx.is_global_override })),
    )
    .await
    .map_err(|e| {
        tracing::error!("Failed to audit org.member.role_updated: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Collect workspace ids inside the transaction so the broker revoke
    // operates on a stable snapshot of the org's workspaces at commit time.
    let org_workspace_ids: Vec<Uuid> = Workspaces::find()
        .filter(workspaces::Column::OrgId.eq(ctx.org.id))
        .select_only()
        .column(workspaces::Column::Id)
        .into_tuple()
        .all(&txn)
        .await
        .map_err(|e| {
            tracing::error!("Failed to list org workspaces: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    txn.commit().await.map_err(|e| {
        tracing::error!("Failed to commit transaction: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Role change → the airhouse role of every outstanding credential the
    // user holds (Owner→Admin, Admin→Writer, Member→Reader) is now wrong.
    // Drain the in-process cache and revoke each cached credential remotely
    // so the OLD role stops authenticating immediately, before the next
    // mint produces a credential under the NEW role. Best-effort; the 24h
    // TTL is the absolute ceiling.
    //
    // Each per-workspace revoke is independent (separate cache key, separate
    // upstream tenant), so fan them out concurrently — the prior sequential
    // loop made the role-change request take O(workspaces) network RTTs.
    if let Some(broker) = airhouse::token_broker() {
        futures::future::join_all(
            org_workspace_ids
                .iter()
                .map(|workspace_id| broker.revoke_user_across_roles(*workspace_id, target_user_id)),
        )
        .await;
    }

    Ok(Json(MemberResponse {
        id: updated.id,
        user_id: updated.user_id,
        email: user.label().to_string(),
        name: user.name,
        role: updated.role.as_str().to_string(),
        created_at: updated.created_at.to_rfc3339(),
    }))
}

/// DELETE /orgs/:org_id/members/:user_id
pub async fn remove_member(
    OrgAdmin(ctx): OrgAdmin,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((_org_id, target_user_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, StatusCode> {
    // Owners cannot remove themselves — use a dedicated "leave org" flow.
    let is_self = target_user_id == ctx.membership.user_id;
    member_authz::authorize_removal_intent(&ctx.membership.role, is_self)?;

    let db = establish_connection().await.map_err(|e| {
        tracing::error!("DB connection error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Transaction with row-level locks on the org's owner rows prevents a
    // TOCTOU where two concurrent removals both see owner_count > 1 and
    // delete the last owner.
    let txn = db.begin().await.map_err(|e| {
        tracing::error!("Failed to start transaction: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let target = OrgMembers::find()
        .filter(org_members::Column::OrgId.eq(ctx.org.id))
        .filter(org_members::Column::UserId.eq(target_user_id))
        .lock_exclusive()
        .one(&txn)
        .await
        .map_err(|e| {
            tracing::error!("Failed to query target member: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    // Only Owners can remove other Owners or Admins.
    member_authz::authorize_target_modification(&ctx.membership.role, &target.role)?;
    let removed_role = target.role.clone();
    if target.role == OrgRole::Owner {
        let owner_count = OrgMembers::find()
            .filter(org_members::Column::OrgId.eq(ctx.org.id))
            .filter(org_members::Column::Role.eq(OrgRole::Owner))
            .lock_exclusive()
            .count(&txn)
            .await
            .map_err(|e| {
                tracing::error!("Failed to count owners: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        if owner_count <= 1 {
            return Err(StatusCode::BAD_REQUEST);
        }
    }

    // Remove workspace-level role overrides for this user scoped to workspaces
    // in this org, so a re-invite doesn't silently reactivate stale overrides.
    let org_workspace_ids: Vec<Uuid> = {
        use entity::prelude::{WorkspaceMembers, Workspaces};
        use entity::workspace_members::Column as WsMemberCol;
        use entity::workspaces::Column as WorkspaceCol;

        let workspace_ids: Vec<Uuid> = Workspaces::find()
            .filter(WorkspaceCol::OrgId.eq(ctx.org.id))
            .select_only()
            .column(WorkspaceCol::Id)
            .into_tuple()
            .all(&txn)
            .await
            .map_err(|e| {
                tracing::error!("Failed to list org workspaces: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

        if !workspace_ids.is_empty() {
            WorkspaceMembers::delete_many()
                .filter(WsMemberCol::UserId.eq(target_user_id))
                .filter(WsMemberCol::WorkspaceId.is_in(workspace_ids.clone()))
                .exec(&txn)
                .await
                .map_err(|e| {
                    tracing::error!("Failed to delete workspace overrides: {e}");
                    StatusCode::INTERNAL_SERVER_ERROR
                })?;
        }

        workspace_ids
    };

    let target_email = Users::find_by_id(target_user_id)
        .one(&txn)
        .await
        .map_err(|e| {
            tracing::error!("Failed to query removed user: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .map(|u| u.email)
        .unwrap_or_default();

    let active: org_members::ActiveModel = target.into();
    active.delete(&txn).await.map_err(|e| {
        tracing::error!("Failed to remove member: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Losing access is as auditable as gaining it — and the removal of an Owner or
    // Admin is precisely the event a compromised-account investigation starts from.
    audit::record_in_txn(
        &txn,
        audit::AuditEntry::new(actor.label().to_string(), "org.member.removed")
            .actor(actor.id, audit::ActorType::User)
            .org(ctx.org.id)
            .target(
                "user",
                target_user_id.to_string(),
                target_email.unwrap_or_default(),
            )
            .change(
                serde_json::json!({ "role": removed_role.as_str() }),
                serde_json::json!(null),
            )
            .metadata(serde_json::json!({ "via_global_override": ctx.is_global_override })),
    )
    .await
    .map_err(|e| {
        tracing::error!("Failed to audit org.member.removed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    txn.commit().await.map_err(|e| {
        tracing::error!("Failed to commit transaction: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // The user is gone from this org — every cached credential belongs to a
    // role they no longer hold. Drop the cache AND remotely revoke each
    // cached credential so they can't keep authenticating with a token
    // they grabbed minutes before being removed. Older mints that have
    // already rotated out of the cache rely on the 24h TTL ceiling
    // (closing that gap requires either an airhouse list endpoint or
    // local mint-row tracking). Fan out concurrently — each workspace's
    // revoke hits a different upstream tenant.
    if let Some(broker) = airhouse::token_broker() {
        futures::future::join_all(
            org_workspace_ids
                .iter()
                .map(|workspace_id| broker.revoke_user_across_roles(*workspace_id, target_user_id)),
        )
        .await;
    }

    // Sync Stripe seat quantity (decrement). Same best-effort pattern as
    // accept_invitation — reconciliation catches any failure.
    if let Ok(svc) = oxy_app::server::api::billing::billing_service().await {
        let org_id_bg = ctx.org.id;
        tokio::spawn(async move {
            if let Err(e) = svc.sync_seats(org_id_bg).await {
                tracing::warn!(?e, ?org_id_bg, "sync_seats failed after remove_member");
            }
        });
    }

    Ok(StatusCode::NO_CONTENT)
}
