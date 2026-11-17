//! `PUT /api/customer-apps/{id}/promotion-rule` — the app's four-eyes rule
//! (`apps.require_second_person_to_promote`), read back on every app response.
//!
//! Guarded like the app's other settings: the console nest's layers
//! (owner-or-app-admin, `Action::PlatformApps`, the grant's app scope, no
//! active assume session), and here a sandbox agent token
//! ([`RefuseSandboxAgent`]) and a publish token are refused. The publish
//! token matters most: the rule exists to stop one identity shipping alone,
//! and CI's token is that identity. (`app_publish_token_scope` already answers
//! a `PUT` from one `403`, and an `app_publish`-granted API token `404`; this
//! says so where the rule lives.)
//!
//! A change writes one audit row, `app.promotion_rule_changed`, with the
//! value before and after. Re-sending the current value writes nothing.

use axum::extract::Path;
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::Utc;
use entity::apps;
use entity::prelude::{Apps, Organizations};
use oxy::database::client::establish_connection;
use oxy_app_core::agent_refusal::RefuseSandboxAgent;
use oxy_app_core::audit::{self, AuditEntry, RequestActor};
use oxy_auth::types::AppPublishTokenAuth;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection, EntityTrait};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use super::dto::{ApiErr, AppResponse};
use super::ops::{api_err, internal};

/// The audit action of a change to the rule.
pub const RULE_CHANGED: &str = "app.promotion_rule_changed";

#[derive(Debug, Deserialize)]
pub struct PromotionRuleRequest {
    pub require_second_person_to_promote: bool,
}

/// `PUT /api/customer-apps/{id}/promotion-rule`: the app, as it now is.
pub async fn set_promotion_rule(
    _: RefuseSandboxAgent,
    actor: RequestActor,
    marker: Option<Extension<AppPublishTokenAuth>>,
    Path(id): Path<Uuid>,
    Json(req): Json<PromotionRuleRequest>,
) -> Result<Json<AppResponse>, ApiErr> {
    if marker.is_some() {
        return Err(api_err(
            StatusCode::FORBIDDEN,
            "a publish token cannot change who may promote this app: sign in as a person",
        ));
    }
    let db = establish_connection().await.map_err(internal)?;
    let app = set_rule(&db, &actor, id, req.require_second_person_to_promote).await?;
    let org = Organizations::find_by_id(app.org_id)
        .one(&db)
        .await
        .map_err(internal)?
        .ok_or_else(|| api_err(StatusCode::INTERNAL_SERVER_ERROR, "Organization not found."))?;
    Ok(Json(AppResponse::from_model_with_org(app, &org.slug)))
}

/// Set the rule to `on` and audit the change; the app unchanged when it
/// already is.
async fn set_rule(
    db: &DatabaseConnection,
    actor: &RequestActor,
    id: Uuid,
    on: bool,
) -> Result<apps::Model, ApiErr> {
    let app = Apps::find_by_id(id)
        .one(db)
        .await
        .map_err(internal)?
        .ok_or_else(|| api_err(StatusCode::NOT_FOUND, "App not found."))?;
    let before = app.require_second_person_to_promote;
    if before == on {
        return Ok(app);
    }
    let (org_id, label) = (app.org_id, format!("{} ({})", app.name, app.slug));
    let mut active: apps::ActiveModel = app.into();
    active.require_second_person_to_promote = ActiveValue::Set(on);
    active.updated_at = ActiveValue::Set(Utc::now().fixed_offset());
    let updated = active.update(db).await.map_err(|e| {
        tracing::error!("promotion rule of app {id} not saved: {e}");
        internal(e)
    })?;
    // The serve path caches the app row. It never reads the rule — the
    // production move reads it from the row it locks — but a cached row
    // should not disagree with the table it came from.
    crate::custom_apps_cache::invalidate_app_resolution_cache_for_app(id);
    audit::record_best_effort(
        db,
        AuditEntry::for_request(actor, RULE_CHANGED)
            .org(org_id)
            .target("app", id.to_string(), label)
            .change(
                json!({ "require_second_person_to_promote": before }),
                json!({ "require_second_person_to_promote": on }),
            ),
    )
    .await;
    Ok(updated)
}
