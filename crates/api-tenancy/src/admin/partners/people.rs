//! `/api/admin/partners/{org_id}/people/{org_member_id}` — the STAFF override for
//! partner access.
//!
//! Normally the partner's own owner/admin decides who is an operator (partner
//! console). Oxy staff can do it here too — to bootstrap or repair a partnership —
//! and every such write is audited with `via_global_override` so "Oxy reached in
//! and changed who can act on your clients" is never silent.
//!
//! Granting access is not billing or secrets, so it stays at the Global-Admin tier
//! (the outer admin guard); no owner escalation.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use entity::partner_role_bindings;
use entity::prelude::{OrgMembers, PartnerGrants, PartnerRoleBindings, Users};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter, TransactionTrait,
};
use uuid::Uuid;

use super::detail::{PartnerDetail, load_detail};
use super::{db, internal};
use oxy_app_core::audit::{self, AuditEntry};

/// `PUT /admin/partners/{org_id}/people/{org_member_id}` — grant partner access as
/// staff. Idempotent.
pub async fn grant_access(
    Path((org_id, org_member_id)): Path<(Uuid, Uuid)>,
    actor: oxy_app_core::audit::RequestActor,
) -> Result<Json<PartnerDetail>, StatusCode> {
    let db = db().await?;
    if PartnerGrants::find_by_id(org_id)
        .one(&db)
        .await
        .map_err(internal("load grant"))?
        .is_none()
    {
        return Err(StatusCode::NOT_FOUND);
    }

    let member = OrgMembers::find_by_id(org_member_id)
        .one(&db)
        .await
        .map_err(internal("load member"))?
        .filter(|m| m.org_id == org_id)
        .ok_or(StatusCode::NOT_FOUND)?;

    let already = PartnerRoleBindings::find()
        .filter(partner_role_bindings::Column::OrgMemberId.eq(org_member_id))
        .one(&db)
        .await
        .map_err(internal("load access row"))?
        .is_some();

    if !already {
        let target_email = Users::find_by_id(member.user_id)
            .one(&db)
            .await
            .map_err(internal("load target user"))?
            .map(|u| u.email)
            .unwrap_or_default();

        let txn = db.begin().await.map_err(internal("begin grant"))?;
        partner_role_bindings::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            org_member_id: ActiveValue::Set(org_member_id),
            created_at: ActiveValue::NotSet,
        }
        .insert(&txn)
        .await
        .map_err(internal("insert access row"))?;

        audit::record_in_txn(
            &txn,
            AuditEntry::for_request(&actor, "partner.access.granted")
                .partner(org_id)
                .org(org_id)
                .target(
                    "partner_member",
                    org_member_id.to_string(),
                    target_email.unwrap_or_default(),
                )
                .metadata(serde_json::json!({ "via_global_override": true })),
        )
        .await
        .map_err(internal("audit access.granted"))?;
        txn.commit().await.map_err(internal("commit grant"))?;
    }

    load_detail(&db, org_id).await.map(Json)
}

/// `DELETE /admin/partners/{org_id}/people/{org_member_id}` — revoke partner access
/// as staff.
pub async fn revoke_access(
    Path((org_id, org_member_id)): Path<(Uuid, Uuid)>,
    actor: oxy_app_core::audit::RequestActor,
) -> Result<Json<PartnerDetail>, StatusCode> {
    let db = db().await?;
    let binding = PartnerRoleBindings::find()
        .filter(partner_role_bindings::Column::OrgMemberId.eq(org_member_id))
        .one(&db)
        .await
        .map_err(internal("load access row"))?
        .ok_or(StatusCode::NOT_FOUND)?;

    // The member must belong to THIS partner org — don't let a stray member id from
    // another org be revoked through this partner's URL.
    let belongs = OrgMembers::find_by_id(org_member_id)
        .one(&db)
        .await
        .map_err(internal("load member"))?
        .is_some_and(|m| m.org_id == org_id);
    if !belongs {
        return Err(StatusCode::NOT_FOUND);
    }

    let txn = db.begin().await.map_err(internal("begin revoke"))?;
    PartnerRoleBindings::delete_by_id(binding.id)
        .exec(&txn)
        .await
        .map_err(internal("delete access row"))?;

    audit::record_in_txn(
        &txn,
        AuditEntry::for_request(&actor, "partner.access.revoked")
            .partner(org_id)
            .org(org_id)
            .target("partner_member", org_member_id.to_string(), String::new())
            .metadata(serde_json::json!({ "via_global_override": true })),
    )
    .await
    .map_err(internal("audit access.revoked"))?;
    txn.commit().await.map_err(internal("commit revoke"))?;

    load_detail(&db, org_id).await.map(Json)
}
