//! `/api/orgs/{org_id}/partner-publish-consent` — the client's opt-in switch for
//! third-party (partner) app publishing.
//!
//! Modelled on the Oxy-staff lockdown (`workspace_oxy_access`), inverted: here the
//! **default is OFF** (no row = a partner cannot publish into this org), and a row
//! with `enabled = true` is the client's explicit consent.
//!
//! **Tenant-sovereign.** Setting it requires a *real* org Owner/Admin — the guard
//! is [`OrgAdminStrict`], which rejects the synthetic-operator override, so neither
//! Oxy staff nor the partner can flip it on the client's behalf. Revoking (setting
//! it back to `false`) denies the partner's next publish immediately, because
//! `custom_apps_publish_authz` reads this at publish time, not at mint time.
//!
//! Every change is audited in the client's own log.

use axum::Json;
use axum::http::StatusCode;
use entity::partner_publish_consent;
use entity::prelude::PartnerPublishConsent;
use oxy::database::client::establish_connection;
use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait, TransactionTrait};
use serde::{Deserialize, Serialize};

use crate::server::api::middlewares::role_guards::{OrgAdmin, OrgAdminStrict};
use oxy_app_core::audit::{self, AuditEntry};

#[derive(Serialize)]
pub struct ConsentStatus {
    /// The current opt-in state. `false` = a partner cannot publish here.
    pub enabled: bool,
    /// Whether THIS caller may change it — a real org officer can; an Oxy operator
    /// viewing via the global override sees the state but the switch is disabled.
    pub can_manage: bool,
}

#[derive(Deserialize)]
pub struct SetConsentBody {
    pub enabled: bool,
}

fn internal<E: std::fmt::Display>(ctx: &'static str) -> impl Fn(E) -> StatusCode {
    move |e| {
        tracing::error!("partner_publish_consent: {ctx}: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

/// `GET /orgs/{org_id}/partner-publish-consent` — visible to any org
/// admin/operator; `can_manage` is false under the global override.
pub async fn get_consent(OrgAdmin(ctx): OrgAdmin) -> Result<Json<ConsentStatus>, StatusCode> {
    let db = establish_connection().await.map_err(internal("db"))?;
    let enabled = PartnerPublishConsent::find_by_id(ctx.org.id)
        .one(&db)
        .await
        .map_err(internal("load consent"))?
        .map(|c| c.enabled)
        .unwrap_or(false);
    Ok(Json(ConsentStatus {
        enabled,
        // A real officer may flip it; the synthetic-override operator may not.
        can_manage: !ctx.is_global_override,
    }))
}

/// `PUT /orgs/{org_id}/partner-publish-consent` — set it. `OrgAdminStrict` rejects
/// the synthetic override, so only a real Owner/Admin of THIS org reaches here.
pub async fn set_consent(
    OrgAdminStrict(ctx): OrgAdminStrict,
    actor: oxy_app_core::audit::RequestActor,
    Json(body): Json<SetConsentBody>,
) -> Result<Json<ConsentStatus>, StatusCode> {
    let db = establish_connection().await.map_err(internal("db"))?;

    let existing = PartnerPublishConsent::find_by_id(ctx.org.id)
        .one(&db)
        .await
        .map_err(internal("load consent"))?;
    let before = existing.as_ref().map(|c| c.enabled).unwrap_or(false);

    // Turning consent on or off is exactly the "who let a third party into my
    // tenant" event the audit chain exists for, so the write and its audit row
    // share one transaction.
    let txn = db.begin().await.map_err(internal("begin"))?;

    let model = partner_publish_consent::ActiveModel {
        org_id: ActiveValue::Set(ctx.org.id),
        enabled: ActiveValue::Set(body.enabled),
        granted_by: ActiveValue::Set(Some(actor.id)),
        updated_at: ActiveValue::Set(chrono::Utc::now().into()),
    };
    if existing.is_some() {
        model.update(&txn).await.map_err(internal("update"))?;
    } else {
        model.insert(&txn).await.map_err(internal("insert"))?;
    }

    let action = if body.enabled {
        "partner_publish.consent.granted"
    } else {
        "partner_publish.consent.revoked"
    };
    audit::record_in_txn(
        &txn,
        AuditEntry::for_request(&actor, action)
            .org(ctx.org.id)
            .target("organization", ctx.org.id.to_string(), ctx.org.name.clone())
            .change(
                serde_json::json!({ "enabled": before }),
                serde_json::json!({ "enabled": body.enabled }),
            ),
    )
    .await
    .map_err(internal("audit"))?;
    txn.commit().await.map_err(internal("commit"))?;

    Ok(Json(ConsentStatus {
        enabled: body.enabled,
        can_manage: true,
    }))
}
