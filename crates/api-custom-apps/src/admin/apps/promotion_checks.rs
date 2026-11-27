//! `PUT /api/customer-apps/{id}/promotion-checks` — which promote checks hold
//! the app's promote to production, and what happens after one
//! (`apps.promotion_checks`, `custom_apps_build_checks::settings`), read back
//! on every app response as `promotion_checks`.
//!
//! The body replaces the stored object; a field it leaves out takes its
//! default:
//!
//! ```json
//! { "required": ["functions", "loads", "design_system", "smoke"],
//!   "seen_required": false, "production_smoke": true, "auto_rollback": false }
//! ```
//!
//! `required: null` is the defaults (every kind that can be required), `[]`
//! none. `validation` is refused: it has its own blocker, which no setting
//! clears. The settings are stored whether or not the org's
//! `promote_checks_gate` flag is on, and read only where it is
//! (`promotion_checks.enabled` on the response).
//!
//! Guarded like the app's four-eyes rule (`promotion_rule`): the console
//! nest's layers (owner-or-app-admin, `Action::PlatformApps`, the grant's app
//! scope, no active assume session), and here a sandbox agent token
//! ([`RefuseSandboxAgent`]) and a publish token are refused: the settings
//! decide what CI's builds must pass, and CI's token is what they hold.
//!
//! Refusals are `{"code", "message"}`: `400 invalid_required`, `403
//! publish_token_refused`, `404 app_not_found`. A change writes one audit
//! row, `app.promotion_checks_changed`, with the settings before and after.
//! Re-sending the current settings writes nothing.

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::Utc;
use entity::apps;
use entity::prelude::{Apps, Organizations};
use oxy::database::client::establish_connection;
use oxy_app_core::agent_refusal::RefuseSandboxAgent;
use oxy_app_core::audit::{self, AuditEntry, RequestActor};
use oxy_auth::types::AppPublishTokenAuth;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection, EntityTrait};
use serde_json::json;
use uuid::Uuid;

use super::dto::AppResponse;
use crate::custom_apps_build_checks::settings::Settings;

/// The audit action of a change to the settings.
pub const CHECKS_CHANGED: &str = "app.promotion_checks_changed";

/// Why the settings were not saved.
#[derive(Debug)]
pub enum Refusal {
    Coded {
        status: StatusCode,
        code: &'static str,
        message: String,
    },
    Internal(String),
}

impl Refusal {
    fn coded(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self::Coded {
            status,
            code,
            message: message.into(),
        }
    }

    fn internal(e: impl std::fmt::Display) -> Self {
        Self::Internal(e.to_string())
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Coded {
                status,
                code,
                message,
            } => (status, code, message),
            Self::Internal(e) => {
                tracing::error!(error = %e, "promotion checks settings not saved");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "internal error".to_string(),
                )
            }
        };
        (status, Json(json!({ "code": code, "message": message }))).into_response()
    }
}

/// `PUT /api/customer-apps/{id}/promotion-checks`: the app, as it now is.
pub async fn set_promotion_checks(
    _: RefuseSandboxAgent,
    actor: RequestActor,
    marker: Option<Extension<AppPublishTokenAuth>>,
    Path(id): Path<Uuid>,
    Json(asked): Json<Settings>,
) -> Result<Json<AppResponse>, Refusal> {
    if marker.is_some() {
        return Err(Refusal::coded(
            StatusCode::FORBIDDEN,
            "publish_token_refused",
            "a publish token cannot change which checks hold this app's promote: sign in as a \
             person",
        ));
    }
    let settings = asked
        .validated()
        .map_err(|why| Refusal::coded(StatusCode::BAD_REQUEST, "invalid_required", why))?;
    let db = establish_connection().await.map_err(Refusal::internal)?;
    let app = save(&db, &actor, id, settings).await?;
    let org = Organizations::find_by_id(app.org_id)
        .one(&db)
        .await
        .map_err(Refusal::internal)?
        .ok_or_else(|| Refusal::internal("the app's organization is gone"))?;
    Ok(Json(AppResponse::from_model_with_org(app, &org.slug)))
}

/// Store `settings` on the app and audit the change; the app unchanged when
/// it already holds them.
async fn save(
    db: &DatabaseConnection,
    actor: &RequestActor,
    id: Uuid,
    settings: Settings,
) -> Result<apps::Model, Refusal> {
    let app = Apps::find_by_id(id)
        .one(db)
        .await
        .map_err(Refusal::internal)?
        .ok_or_else(|| Refusal::coded(StatusCode::NOT_FOUND, "app_not_found", "no such app"))?;
    let before = Settings::of(&app);
    // A null column already reads as the defaults, and is left null for them.
    if before == settings {
        return Ok(app);
    }
    let stored = serde_json::to_value(&settings).map_err(Refusal::internal)?;
    let (org_id, label) = (app.org_id, format!("{} ({})", app.name, app.slug));
    let mut active: apps::ActiveModel = app.into();
    active.promotion_checks = ActiveValue::Set(Some(stored.clone()));
    active.updated_at = ActiveValue::Set(Utc::now().fixed_offset());
    let updated = active.update(db).await.map_err(Refusal::internal)?;
    // The serve path caches the app row and never reads the settings, but a
    // cached row should not disagree with the table it came from.
    crate::custom_apps_cache::invalidate_app_resolution_cache_for_app(id);
    audit::record_best_effort(
        db,
        AuditEntry::for_request(actor, CHECKS_CHANGED)
            .org(org_id)
            .target("app", id.to_string(), label)
            .change(serde_json::to_value(&before).unwrap_or_default(), stored),
    )
    .await;
    Ok(updated)
}
