//! App-scoped publish tokens a partner mints for a **specific client app** it
//! manages — the partner-scoped mirror of the admin publish-tokens surface.
//!
//! Safe by construction. The token carries `app_id`, so the publish path
//! authorizes by "token's app == target app AND the client consents" and returns
//! with **no fallthrough** to the minter's other authority (see
//! `custom_apps_publish::authorize_publish`). So the token can publish to that
//! ONE app and nowhere else, whatever else the operator manages. `created_by` is
//! the minting operator (attribution + revocation), and the client's
//! `partner_publish_consent` must be ON to mint — and is re-checked at publish, so
//! a later consent-off or a revoke denies the next publish.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use chrono::Utc;
use entity::prelude::{AppPublishTokens, Apps};
use entity::{app_publish_tokens, apps};
use oxy_auth::app_publish_token_domain::generate_token;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter, QueryOrder,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{db, internal, require_org_scope};
use crate::partner_console::partner_context::PartnerActor;
use oxy_app::server::api::custom_apps_publish_authz::consent_enabled;
use oxy_app_core::audit::{self, ActorType, AuditEntry};
use oxy_server_authz::partner_authz::{PartnerCapability, PartnerScope};

/// A token this app is allowed to have exists at most 90 days — a partner's CI
/// credential for a client's app should not outlive review.
const TOKEN_TTL_DAYS: i64 = 90;

#[derive(Deserialize)]
pub struct CreateBody {
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Serialize)]
pub struct CreatedToken {
    pub id: Uuid,
    /// Plaintext — shown once. Paste into a CI secret as `OXY_TOKEN`.
    pub token: String,
    pub name: String,
    pub token_prefix: String,
    pub created_at: String,
    pub expires_at: String,
}

#[derive(Serialize)]
pub struct TokenDto {
    pub id: Uuid,
    pub name: String,
    pub token_prefix: String,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub last_used_at: Option<String>,
}

/// Resolve `app_id` → its org and require the partner to manage that org with the
/// app/data-plane capability. `require_org_scope` 404s an org outside the partner's
/// set, so this also refuses an app the partner doesn't manage.
async fn require_managed_app(
    db: &sea_orm::DatabaseConnection,
    scope: &PartnerScope,
    app_id: Uuid,
) -> Result<apps::Model, StatusCode> {
    let app = Apps::find_by_id(app_id)
        .one(db)
        .await
        .map_err(internal("load app"))?
        .ok_or(StatusCode::NOT_FOUND)?;
    require_org_scope(db, scope, app.org_id, PartnerCapability::ManageApps).await?;
    Ok(app)
}

/// `GET /partners/{id}/apps/{app_id}/publish-tokens` — live tokens for one client app.
pub async fn list_tokens(
    PartnerActor(scope): PartnerActor,
    Path((_partner_org_id, app_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<TokenDto>>, StatusCode> {
    let db = db().await?;
    require_managed_app(&db, &scope, app_id).await?;

    let rows = AppPublishTokens::find()
        .filter(app_publish_tokens::Column::AppId.eq(app_id))
        .filter(app_publish_tokens::Column::RevokedAt.is_null())
        .order_by_desc(app_publish_tokens::Column::CreatedAt)
        .all(&db)
        .await
        .map_err(internal("list tokens"))?;

    let out = rows
        .into_iter()
        .map(|t| TokenDto {
            id: t.id,
            name: t.name,
            token_prefix: t.token_prefix,
            created_at: t.created_at.to_rfc3339(),
            expires_at: t.expires_at.map(|e| e.to_rfc3339()),
            last_used_at: t.last_used_at.map(|e| e.to_rfc3339()),
        })
        .collect();
    Ok(Json(out))
}

/// `POST /partners/{id}/apps/{app_id}/publish-tokens` — mint an app-scoped token.
pub async fn create_token(
    PartnerActor(scope): PartnerActor,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((_partner_org_id, app_id)): Path<(Uuid, Uuid)>,
    body: Option<Json<CreateBody>>,
) -> Result<Json<CreatedToken>, StatusCode> {
    let db = db().await?;
    let app = require_managed_app(&db, &scope, app_id).await?;

    // The client must have partner publishing ON. Minting a credential a client
    // hasn't consented to would be presumptuous — and it wouldn't work anyway
    // (publish re-checks consent), so fail fast with a clear signal.
    if !consent_enabled(&db, app.org_id).await {
        return Err(StatusCode::FORBIDDEN);
    }

    let name = body
        .and_then(|Json(b)| b.name)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("{} publish {}", app.name, Utc::now().format("%Y-%m-%d")));

    let generated = generate_token();
    let now = Utc::now().fixed_offset();
    let expires = now + chrono::Duration::days(TOKEN_TTL_DAYS);
    let id = Uuid::new_v4();

    let txn = db.begin().await.map_err(internal("begin mint"))?;
    app_publish_tokens::ActiveModel {
        id: ActiveValue::Set(id),
        name: ActiveValue::Set(name.clone()),
        token_hash: ActiveValue::Set(generated.token_hash),
        token_prefix: ActiveValue::Set(generated.token_prefix.clone()),
        created_by: ActiveValue::Set(Some(actor.id)),
        created_at: ActiveValue::Set(now),
        last_used_at: ActiveValue::Set(None),
        revoked_at: ActiveValue::Set(None),
        // BOTH set: `app_id` confines the token to this one app at publish time;
        // `created_by` (above) keeps it attributable and revocable.
        app_id: ActiveValue::Set(Some(app_id)),
        expires_at: ActiveValue::Set(Some(expires)),
    }
    .insert(&txn)
    .await
    .map_err(internal("insert token"))?;

    audit::record_in_txn(
        &txn,
        AuditEntry::new(actor.label().to_string(), "partner.publish_token.minted")
            .actor(actor.id, ActorType::User)
            .partner(scope.partner_id)
            .org(app.org_id)
            .target("app", app_id.to_string(), app.name.clone()),
    )
    .await
    .map_err(internal("audit mint"))?;
    txn.commit().await.map_err(internal("commit mint"))?;

    Ok(Json(CreatedToken {
        id,
        token: generated.plaintext,
        name,
        token_prefix: generated.token_prefix,
        created_at: now.to_rfc3339(),
        expires_at: expires.to_rfc3339(),
    }))
}

/// `DELETE /partners/{id}/apps/{app_id}/publish-tokens/{token_id}` — revoke.
pub async fn revoke_token(
    PartnerActor(scope): PartnerActor,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((_partner_org_id, app_id, token_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<StatusCode, StatusCode> {
    let db = db().await?;
    let app = require_managed_app(&db, &scope, app_id).await?;

    let token = AppPublishTokens::find_by_id(token_id)
        .one(&db)
        .await
        .map_err(internal("load token"))?
        // The token must belong to THIS app — a partner can only revoke tokens for
        // apps it manages, never reach across to another app's credential by id.
        .filter(|t| t.app_id == Some(app_id))
        .ok_or(StatusCode::NOT_FOUND)?;

    if token.revoked_at.is_some() {
        return Ok(StatusCode::NO_CONTENT);
    }

    let txn = db.begin().await.map_err(internal("begin revoke"))?;
    let mut active: app_publish_tokens::ActiveModel = token.into();
    active.revoked_at = ActiveValue::Set(Some(Utc::now().fixed_offset()));
    active
        .update(&txn)
        .await
        .map_err(internal("revoke token"))?;

    audit::record_in_txn(
        &txn,
        AuditEntry::new(actor.label().to_string(), "partner.publish_token.revoked")
            .actor(actor.id, ActorType::User)
            .partner(scope.partner_id)
            .org(app.org_id)
            .target("app_publish_token", token_id.to_string(), app.name.clone()),
    )
    .await
    .map_err(internal("audit revoke"))?;
    txn.commit().await.map_err(internal("commit revoke"))?;

    Ok(StatusCode::NO_CONTENT)
}
