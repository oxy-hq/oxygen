//! `POST /api/admin/partners/grant` — the **atomic** grant-partnership flow.
//!
//! Granting a partnership means: *this existing org may now administer other
//! orgs.* There is no separate partner entity to create — the partner IS the org
//! (see `internal-docs/partner-platform.md` §2), so its name,
//! slug and people all come from the org that already exists.
//!
//! The whole grant is ONE transaction — grant row, ceiling, first client
//! attachment, first partner admin — and its audit rows are written with
//! [`audit::record_in_txn`], so the mutation and its tamper-evident entries
//! commit or roll back together. A grant can no longer land with its "who gave
//! this partner power over my org" audit row silently dropped.

use axum::Json;
use axum::http::StatusCode;
use entity::prelude::{OrgMembers, Organizations, PartnerGrants, PartnerOrgs, Users};
use entity::{org_members, partner_orgs, partner_role_bindings};
use oxy::database::filters::UserQueryFilterExt;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter, TransactionTrait,
};
use serde::Deserialize;
use uuid::Uuid;

use super::{
    CapabilitiesInput, PartnerDetail, ceiling_model, db, grant_model, internal, load_detail,
    require_owner_for_sensitive_caps,
};
use oxy_app_core::audit::{self, AuditEntry};

#[derive(Deserialize)]
pub struct GrantBody {
    /// The org that becomes a partner. It must already exist — a partner is a
    /// real tenant, not a shell.
    pub partner_org_id: Uuid,
    /// The ceiling. Defaults to least privilege.
    pub capabilities: Option<CapabilitiesInput>,
    /// Optional: attach the first client in the same transaction.
    pub first_client_org_id: Option<Uuid>,
    /// Optional: name the partner's own boss. Must already be a member of the
    /// partner org — org invitations cover the not-yet-signed-up case.
    pub partner_admin_email: Option<String>,
}

pub async fn grant_partnership(
    actor: oxy_app_core::audit::RequestActor,
    Json(body): Json<GrantBody>,
) -> Result<Json<PartnerDetail>, StatusCode> {
    let db = db().await?;

    let partner_org = Organizations::find_by_id(body.partner_org_id)
        .one(&db)
        .await
        .map_err(internal("load partner org"))?
        .ok_or(StatusCode::NOT_FOUND)?;

    // Sub-partners are disallowed (cycle risk, no demand — design §7): an org
    // that is already SOMEONE ELSE'S client cannot itself become a partner.
    if PartnerOrgs::find()
        .filter(partner_orgs::Column::ManagedOrgId.eq(body.partner_org_id))
        .one(&db)
        .await
        .map_err(internal("sub-partner check"))?
        .is_some()
    {
        return Err(StatusCode::CONFLICT);
    }

    let caps = body
        .capabilities
        .clone()
        .unwrap_or_else(CapabilitiesInput::sane_default);
    require_owner_for_sensitive_caps(&actor, caps.manage_billing, caps.manage_secrets)?;

    // A client already managed by a DIFFERENT partner is a conflict
    // (`partner_orgs.managed_org_id` is UNIQUE). Check before opening the txn.
    if let Some(client_id) = body.first_client_org_id {
        if client_id == body.partner_org_id {
            return Err(StatusCode::BAD_REQUEST);
        }
        if PartnerOrgs::find()
            .filter(partner_orgs::Column::ManagedOrgId.eq(client_id))
            .one(&db)
            .await
            .map_err(internal("attach check"))?
            .is_some()
        {
            return Err(StatusCode::CONFLICT);
        }
        if Organizations::find_by_id(client_id)
            .one(&db)
            .await
            .map_err(internal("load client org"))?
            .is_none()
        {
            return Err(StatusCode::NOT_FOUND);
        }
    }

    // The first partner admin must already belong to the partner org. We do NOT
    // resurrect the email-keyed shadow membership the old `partner_members`
    // table used — a partner's people are org members, full stop.
    let admin_member = match body.partner_admin_email.as_deref().map(str::trim) {
        Some(e) if !e.is_empty() => {
            let user = Users::find()
                .filter_by_email(e)
                .one(&db)
                .await
                .map_err(internal("load admin user"))?
                .ok_or(StatusCode::BAD_REQUEST)?;
            let member = OrgMembers::find()
                .filter(org_members::Column::OrgId.eq(body.partner_org_id))
                .filter(org_members::Column::UserId.eq(user.id))
                .one(&db)
                .await
                .map_err(internal("load admin membership"))?
                // Not a member of the partner org: invite them there first.
                .ok_or(StatusCode::BAD_REQUEST)?;
            Some((member.id, user.email))
        }
        _ => None,
    };

    let txn = db.begin().await.map_err(internal("begin grant"))?;

    // 1. The grant + its ceiling. Idempotent: re-granting an existing partner
    //    just re-affirms it rather than 500ing on a PK collision.
    let already = PartnerGrants::find_by_id(body.partner_org_id)
        .one(&txn)
        .await
        .map_err(internal("load grant"))?;
    if already.is_none() {
        grant_model(body.partner_org_id, actor.id)
            .insert(&txn)
            .await
            .map_err(internal("insert grant"))?;
        ceiling_model(body.partner_org_id, &caps)
            .insert(&txn)
            .await
            .map_err(internal("insert ceiling"))?;

        audit::record_in_txn(
            &txn,
            AuditEntry::for_request(&actor, "partner.granted")
                .partner(body.partner_org_id)
                .org(body.partner_org_id)
                .target(
                    "organization",
                    body.partner_org_id.to_string(),
                    partner_org.name.clone(),
                )
                .metadata(serde_json::json!({ "capabilities": caps })),
        )
        .await
        .map_err(internal("audit partner.granted"))?;
    }

    // 2. First client.
    if let Some(client_id) = body.first_client_org_id {
        partner_orgs::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            partner_org_id: ActiveValue::Set(body.partner_org_id),
            managed_org_id: ActiveValue::Set(client_id),
            created_by: ActiveValue::Set(Some(actor.id)),
            created_at: ActiveValue::NotSet,
        }
        .insert(&txn)
        .await
        .map_err(|_| StatusCode::CONFLICT)?;

        audit::record_in_txn(
            &txn,
            AuditEntry::for_request(&actor, "partner.org.attached")
                .partner(body.partner_org_id)
                .org(client_id)
                .target("organization", client_id.to_string(), String::new()),
        )
        .await
        .map_err(internal("audit partner.org.attached"))?;
    }

    // 3. The first operator — so the partnership isn't stranded with no one able to
    //    reach its console. The partner org's owner/admin grants access to everyone
    //    else, within the ceiling.
    if let Some((member_id, email)) = admin_member {
        let bound = entity::prelude::PartnerRoleBindings::find()
            .filter(partner_role_bindings::Column::OrgMemberId.eq(member_id))
            .one(&txn)
            .await
            .map_err(internal("binding check"))?
            .is_some();
        if !bound {
            partner_role_bindings::ActiveModel {
                id: ActiveValue::Set(Uuid::new_v4()),
                org_member_id: ActiveValue::Set(member_id),
                created_at: ActiveValue::NotSet,
            }
            .insert(&txn)
            .await
            .map_err(internal("insert access row"))?;

            audit::record_in_txn(
                &txn,
                AuditEntry::for_request(&actor, "partner.access.granted")
                    .partner(body.partner_org_id)
                    .org(body.partner_org_id)
                    .target(
                        "partner_member",
                        member_id.to_string(),
                        email.unwrap_or_default(),
                    )
                    .metadata(serde_json::json!({ "via_global_override": true })),
            )
            .await
            .map_err(internal("audit partner.access.granted"))?;
        }
    }

    txn.commit().await.map_err(internal("commit grant"))?;
    load_detail(&db, body.partner_org_id).await.map(Json)
}
