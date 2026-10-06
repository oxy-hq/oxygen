//! Attaching / detaching a partner's client orgs — **Oxy staff only**.
//!
//! Attaching an *existing* org to a partner is staff-only because it hands
//! administration of a live tenant to a third party; that is a contract event.
//! (A partner CAN create a brand-new client itself — see
//! `partner_console::orgs` — because nobody else's tenant is affected.)
//!
//! Detach is likewise staff-only, and deliberately so: a partner must not be
//! able to unilaterally orphan a customer (design §3, invariant 5).

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use entity::partner_orgs;
use entity::prelude::{Organizations, PartnerGrants, PartnerOrgs};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter, TransactionTrait,
};
use serde::Deserialize;
use uuid::Uuid;

use super::{PartnerDetail, db, internal, load_detail};
use oxy_app_core::audit::{self, AuditEntry};

#[derive(Deserialize)]
pub struct AttachBody {
    pub managed_org_id: Uuid,
}

/// `POST /admin/partners/{org_id}/orgs`
pub async fn attach_org(
    Path(partner_org_id): Path<Uuid>,
    actor: oxy_app_core::audit::RequestActor,
    Json(body): Json<AttachBody>,
) -> Result<Json<PartnerDetail>, StatusCode> {
    if body.managed_org_id == partner_org_id {
        // A partner managing itself would make its members' partner role a
        // second, uncapped path into their own org. Never.
        return Err(StatusCode::BAD_REQUEST);
    }
    let db = db().await?;

    if PartnerGrants::find_by_id(partner_org_id)
        .one(&db)
        .await
        .map_err(internal("load grant"))?
        .is_none()
    {
        return Err(StatusCode::NOT_FOUND);
    }
    let org = Organizations::find_by_id(body.managed_org_id)
        .one(&db)
        .await
        .map_err(internal("load org"))?
        .ok_or(StatusCode::NOT_FOUND)?;

    // Sub-partners are disallowed: a partner cannot become someone's client.
    if PartnerGrants::find_by_id(body.managed_org_id)
        .one(&db)
        .await
        .map_err(internal("sub-partner check"))?
        .is_some()
    {
        return Err(StatusCode::CONFLICT);
    }

    match PartnerOrgs::find()
        .filter(partner_orgs::Column::ManagedOrgId.eq(body.managed_org_id))
        .one(&db)
        .await
        .map_err(internal("attach check"))?
    {
        // Already ours — idempotent.
        Some(link) if link.partner_org_id == partner_org_id => {
            return load_detail(&db, partner_org_id).await.map(Json);
        }
        // Someone else's client.
        Some(_) => return Err(StatusCode::CONFLICT),
        None => {}
    }

    let txn = db.begin().await.map_err(internal("begin attach"))?;
    partner_orgs::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        partner_org_id: ActiveValue::Set(partner_org_id),
        managed_org_id: ActiveValue::Set(body.managed_org_id),
        created_by: ActiveValue::Set(Some(actor.id)),
        created_at: ActiveValue::NotSet,
    }
    .insert(&txn)
    .await
    .map_err(|_| StatusCode::CONFLICT)?;

    audit::record_in_txn(
        &txn,
        AuditEntry::for_request(&actor, "partner.org.attached")
            .partner(partner_org_id)
            .org(body.managed_org_id)
            .target("organization", body.managed_org_id.to_string(), org.name),
    )
    .await
    .map_err(internal("audit attach"))?;
    txn.commit().await.map_err(internal("commit attach"))?;

    load_detail(&db, partner_org_id).await.map(Json)
}

/// `DELETE /admin/partners/{org_id}/orgs/{managed_org_id}`
pub async fn detach_org(
    Path((partner_org_id, managed_org_id)): Path<(Uuid, Uuid)>,
    actor: oxy_app_core::audit::RequestActor,
) -> Result<StatusCode, StatusCode> {
    let db = db().await?;
    let link = PartnerOrgs::find()
        .filter(partner_orgs::Column::PartnerOrgId.eq(partner_org_id))
        .filter(partner_orgs::Column::ManagedOrgId.eq(managed_org_id))
        .one(&db)
        .await
        .map_err(internal("load link"))?
        .ok_or(StatusCode::NOT_FOUND)?;

    let txn = db.begin().await.map_err(internal("begin detach"))?;
    PartnerOrgs::delete_by_id(link.id)
        .exec(&txn)
        .await
        .map_err(internal("detach"))?;

    audit::record_in_txn(
        &txn,
        AuditEntry::for_request(&actor, "partner.org.detached")
            .partner(partner_org_id)
            .org(managed_org_id)
            .target("organization", managed_org_id.to_string(), String::new()),
    )
    .await
    .map_err(internal("audit detach"))?;
    txn.commit().await.map_err(internal("commit detach"))?;

    Ok(StatusCode::NO_CONTENT)
}
