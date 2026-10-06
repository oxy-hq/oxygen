//! `/api/admin/partners` — Oxy staff granting and governing partnerships.
//!
//! A partner is **an org that holds a grant** (`partner_grants`). Staff:
//!   * grant a partnership to an existing org (`grant.rs`),
//!   * set the **ceiling** — what that partner may EVER do,
//!   * attach / detach client orgs (`membership.rs`),
//!   * revoke the grant.
//!
//! Staff do NOT hand out partner ROLES — that is the partner's own admin's job,
//! within the ceiling (see `partner_console::people`). This is the two-level
//! governance the design turns on: Oxy caps the partner; the partner staffs itself.
//!
//! Runs under the admin surface's default guard (OXY_OWNER **or** app_admins) —
//! provisioning is an ops action. Granting the sensitive `manage_billing` /
//! `manage_secrets` ceiling flags stays **Owner-only**
//! (`require_owner_for_sensitive_caps`), since opening this router to app_admins
//! would otherwise let a Global Admin mint a partner with billing reach.

mod detail;
mod grant;
mod membership;
mod people;

pub(super) use detail::{
    CapabilitiesDto, CapabilitiesInput, PartnerDetail, PartnerSummary, load_detail,
};

use axum::extract::Path;
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use entity::prelude::{Organizations, PartnerCapabilities, PartnerGrants, PartnerOrgs};
use entity::{organizations, partner_capabilities, partner_grants};
use oxy::database::client::establish_connection;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
    TransactionTrait,
};
use serde_json::json;
use std::collections::HashMap;
use uuid::Uuid;

use crate::server::router::AppState;
use oxy_app_core::audit::{self, AuditEntry};

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/partners", get(list_partners))
        // Grant a partnership to an EXISTING org (atomic: grant + ceiling + first
        // partner admin + optional first client).
        .route(
            "/partners/grant",
            axum::routing::post(grant::grant_partnership),
        )
        .route(
            "/partners/{org_id}",
            get(get_partner).delete(revoke_partner),
        )
        // The CEILING.
        .route("/partners/{org_id}/capabilities", put(set_capabilities))
        .route(
            "/partners/{org_id}/orgs",
            axum::routing::post(membership::attach_org),
        )
        // Detach is Oxy-only: a partner must not be able to orphan a customer.
        .route(
            "/partners/{org_id}/orgs/{managed_org_id}",
            axum::routing::delete(membership::detach_org),
        )
        // Staff override for partner access — grant/revoke who is an operator.
        // Audited via_global_override. Grant is not billing/secrets, so it stays at
        // the Global-Admin tier (no owner escalation).
        .route(
            "/partners/{org_id}/people/{org_member_id}",
            axum::routing::put(people::grant_access).delete(people::revoke_access),
        )
}

// ── shared helpers ─────────────────────────────────────────────────────────

pub(super) async fn db() -> Result<DatabaseConnection, StatusCode> {
    establish_connection().await.map_err(|e| {
        tracing::error!("admin/partners: DB connect failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

pub(super) fn internal<E: std::fmt::Display>(ctx: &str) -> impl Fn(E) -> StatusCode + '_ {
    move |e| {
        tracing::error!("admin/partners: {ctx}: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

/// The two high-sensitivity ceiling flags stay **Owner-only**, even though the rest
/// of provisioning is open to Global Admins. Handing a partner billing or secrets
/// power over a tenant is platform *governance*, not ops.
pub(super) fn require_owner_for_sensitive_caps(
    actor: &oxy_auth::types::AuthenticatedUser,
    manage_billing: bool,
    manage_secrets: bool,
) -> Result<(), StatusCode> {
    let caller = crate::server::authz::Caller::from_user(actor);
    if (manage_billing || manage_secrets)
        && !crate::server::authz::globals::is_global_owner(&caller)
    {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

// ── handlers ────────────────────────────────────────────────────────────

pub async fn list_partners() -> Result<Json<Vec<PartnerSummary>>, StatusCode> {
    let db = db().await?;
    let grants = PartnerGrants::find()
        .all(&db)
        .await
        .map_err(internal("list grants"))?;
    if grants.is_empty() {
        return Ok(Json(vec![]));
    }

    let org_ids: Vec<Uuid> = grants.iter().map(|g| g.org_id).collect();
    let orgs: HashMap<Uuid, organizations::Model> = Organizations::find()
        .filter(organizations::Column::Id.is_in(org_ids))
        .all(&db)
        .await
        .map_err(internal("load partner orgs"))?
        .into_iter()
        .map(|o| (o.id, o))
        .collect();

    let mut counts: HashMap<Uuid, usize> = HashMap::new();
    for row in PartnerOrgs::find()
        .all(&db)
        .await
        .map_err(internal("count clients"))?
    {
        *counts.entry(row.partner_org_id).or_default() += 1;
    }

    let mut out: Vec<PartnerSummary> = grants
        .into_iter()
        .filter_map(|g| {
            let org = orgs.get(&g.org_id)?;
            Some(PartnerSummary {
                managed_count: counts.get(&g.org_id).copied().unwrap_or(0),
                org_id: g.org_id,
                name: org.name.clone(),
                slug: org.slug.clone(),
                status: g.status,
                created_at: g.created_at.to_rfc3339(),
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Json(out))
}

pub async fn get_partner(Path(org_id): Path<Uuid>) -> Result<Json<PartnerDetail>, StatusCode> {
    let db = db().await?;
    load_detail(&db, org_id).await.map(Json)
}

/// `PUT /admin/partners/{org_id}/capabilities` — set the CEILING.
pub async fn set_capabilities(
    Path(org_id): Path<Uuid>,
    actor: oxy_app_core::audit::RequestActor,
    Json(input): Json<CapabilitiesInput>,
) -> Result<Json<CapabilitiesDto>, StatusCode> {
    let db = db().await?;
    if PartnerGrants::find_by_id(org_id)
        .one(&db)
        .await
        .map_err(internal("load grant"))?
        .is_none()
    {
        return Err(StatusCode::NOT_FOUND);
    }
    require_owner_for_sensitive_caps(&actor, input.manage_billing, input.manage_secrets)?;

    let existing = PartnerCapabilities::find_by_id(org_id)
        .one(&db)
        .await
        .map_err(internal("load ceiling"))?;
    let before = existing
        .as_ref()
        .map(|m| json!(CapabilitiesDto::from(m.clone())))
        .unwrap_or(json!(null));

    let model = partner_capabilities::ActiveModel {
        org_id: ActiveValue::Set(org_id),
        manage_members: ActiveValue::Set(input.manage_members),
        manage_apps: ActiveValue::Set(input.manage_apps),
        develop_apps: ActiveValue::Set(input.develop_apps),
        view_audit: ActiveValue::Set(input.view_audit),
        manage_billing: ActiveValue::Set(input.manage_billing),
        manage_secrets: ActiveValue::Set(input.manage_secrets),
        create_orgs: ActiveValue::Set(input.create_orgs),
        manage_org_settings: ActiveValue::Set(input.manage_org_settings),
        updated_at: ActiveValue::Set(chrono::Utc::now().into()),
    };

    // Raising or lowering a partner's ceiling is exactly the "who gave this partner
    // power over my org" event the tamper-evident chain exists for — so the write
    // and its audit row share ONE transaction.
    let txn = db.begin().await.map_err(internal("begin"))?;
    let saved = if existing.is_some() {
        model
            .update(&txn)
            .await
            .map_err(internal("update ceiling"))?
    } else {
        model
            .insert(&txn)
            .await
            .map_err(internal("insert ceiling"))?
    };
    let after = CapabilitiesDto::from(saved);

    audit::record_in_txn(
        &txn,
        AuditEntry::for_request(&actor, "partner.capabilities.updated")
            .partner(org_id)
            .org(org_id)
            .target("partner", org_id.to_string(), String::new())
            .change(before, json!(after)),
    )
    .await
    .map_err(internal("audit ceiling"))?;
    txn.commit().await.map_err(internal("commit"))?;

    Ok(Json(after))
}

/// `DELETE /admin/partners/{org_id}` — revoke the partnership. The ORG survives
/// (it's a real tenant); only its right to manage others is withdrawn.
pub async fn revoke_partner(
    Path(org_id): Path<Uuid>,
    actor: oxy_app_core::audit::RequestActor,
) -> Result<StatusCode, StatusCode> {
    let db = db().await?;
    let txn = db.begin().await.map_err(internal("begin"))?;

    PartnerGrants::delete_by_id(org_id)
        .exec(&txn)
        .await
        .map_err(internal("revoke"))?;

    audit::record_in_txn(
        &txn,
        AuditEntry::for_request(&actor, "partner.revoked")
            .partner(org_id)
            .org(org_id)
            .target("partner", org_id.to_string(), String::new()),
    )
    .await
    .map_err(internal("audit revoke"))?;
    txn.commit().await.map_err(internal("commit"))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Shared by `grant.rs`: write the ceiling row.
pub(super) fn ceiling_model(
    org_id: Uuid,
    caps: &CapabilitiesInput,
) -> partner_capabilities::ActiveModel {
    partner_capabilities::ActiveModel {
        org_id: ActiveValue::Set(org_id),
        manage_members: ActiveValue::Set(caps.manage_members),
        manage_apps: ActiveValue::Set(caps.manage_apps),
        develop_apps: ActiveValue::Set(caps.develop_apps),
        view_audit: ActiveValue::Set(caps.view_audit),
        manage_billing: ActiveValue::Set(caps.manage_billing),
        manage_secrets: ActiveValue::Set(caps.manage_secrets),
        create_orgs: ActiveValue::Set(caps.create_orgs),
        manage_org_settings: ActiveValue::Set(caps.manage_org_settings),
        updated_at: ActiveValue::NotSet,
    }
}

/// Shared by `grant.rs`: the grant row.
pub(super) fn grant_model(org_id: Uuid, actor: Uuid) -> partner_grants::ActiveModel {
    partner_grants::ActiveModel {
        org_id: ActiveValue::Set(org_id),
        status: ActiveValue::Set("active".to_string()),
        created_by: ActiveValue::Set(Some(actor)),
        created_at: ActiveValue::NotSet,
    }
}
