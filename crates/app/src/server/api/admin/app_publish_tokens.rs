//! `/api/admin/app-publish-tokens` — CRUD for app publish tokens (machine-auth bearer
//! credentials, primarily for `oxyc publish` in CI).
//!
//! Sits behind the `/admin` door plus `Cap::ManageApps` (publishing is what these are
//! for). The plaintext is returned **once** on create and never stored; only a SHA-256
//! hash and a non-secret display prefix are persisted.
//!
//! ## Tokens are owned by their minter
//!
//! This module used to state the opposite — "managed across admins, not owned by their
//! minter" — and that was a fair rule while every admin was the same homogeneous staff
//! population, all equally entitled to everything.
//!
//! The capability split falsified that premise. `Cap::ManageApps` is now held by App
//! Operators, who never existed when the rule was written, so an unfiltered list plus an
//! id-addressed revoke means a grant bounded to a single tenant can enumerate and revoke
//! **every Oxy engineer's CI publish token** — a cross-operator denial of service with no
//! boundary at all.
//!
//! So list and revoke are scoped to the caller's own tokens, and the shared cross-admin
//! view is what `Cap::OperatePlatform` buys — the capability that already means "operates
//! Oxy's own machinery". Minting needs no boundary of *reach*: a staff token carries
//! `app_id: None`, and `custom_apps_publish_authz::resolve_actor` re-resolves the minter's
//! own capability and scope at publish time, so a token can never out-reach the person
//! holding it.
//!
//! ## A token cannot mint one
//!
//! Reach is not the only thing a credential has; it also has an end. A publish token
//! minted here never expires and names no app, so an API token allowed to mint one could
//! turn its own hours or days into a credential that outlives it — a credential quietly
//! minting a stronger one, which the API-tokens design forbids (§4.6). Minting therefore
//! takes a browser session or a **legacy API key** ([`SessionOrLegacyKey`]): CI scripts
//! mint with a legacy key today and nothing may break one, while every new-format token
//! (`oxy_pat_`, `oxy_sat_`, `oxy_ci_`, `oxy_sbx_`) is answered `403 session_required`,
//! as on the token-management routes.
//!
//! ## Mint and revoke are audited
//!
//! One `audit_events` row each ([`MINTED`], [`REVOKED`]), written in the transaction
//! that changes the token, so neither can happen unrecorded. Built with
//! `AuditEntry::for_request`, so a mint by a legacy key names that key. The row carries
//! the token's id, label and non-secret display prefix — never the plaintext or its hash.
//!
//! A live token authenticates as its minting app-admin **only on the
//! customer-apps admin surface** — see the `app_publish_token_scope` middleware and
//! `oxy_auth::app_publish_token_domain` for enforcement. This module owns lifecycle
//! (create/list/revoke); token generation + hashing live in the auth crate so
//! the acceptance path and the mint path share one implementation.

use axum::Json;
use axum::Router;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::routing::{get, post};
use chrono::Utc;
use entity::app_publish_tokens;
use entity::prelude::AppPublishTokens;
use oxy::database::client::establish_connection;
use oxy_app_core::audit::{self, AuditEntry, RequestActor};
use oxy_auth::app_publish_token_domain::generate_token;
use oxy_auth::extractor::{
    AuthenticatedUserExtractor, SESSION_REQUIRED, SessionAction, SessionOrLegacyKey,
};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter, QueryOrder,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::server::router::AppState;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/app-publish-tokens", get(list_tokens).post(create_token))
        .route("/app-publish-tokens/{id}/revoke", post(revoke_token))
}

#[derive(Debug, Deserialize)]
pub struct CreateTokenBody {
    /// Human-readable label, e.g. "ci-publish". Falls back to a timestamped
    /// default when omitted/blank.
    pub name: Option<String>,
}

/// Create response: the **only** time the plaintext is ever returned. The
/// caller must copy it now; it cannot be retrieved later.
#[derive(Debug, Serialize)]
pub struct CreateTokenResponse {
    pub id: Uuid,
    /// Plaintext token — shown once. Paste into a CI secret as `OXY_TOKEN`.
    pub token: String,
    pub name: String,
    pub token_prefix: String,
    pub created_at: String,
}

/// Metadata-only view for listing — never carries the plaintext or hash.
#[derive(Debug, Serialize)]
pub struct TokenResponse {
    pub id: Uuid,
    pub name: String,
    pub token_prefix: String,
    pub created_by: Option<Uuid>,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked: bool,
    pub revoked_at: Option<String>,
}

impl From<app_publish_tokens::Model> for TokenResponse {
    fn from(m: app_publish_tokens::Model) -> Self {
        Self {
            id: m.id,
            name: m.name,
            token_prefix: m.token_prefix,
            created_by: m.created_by,
            created_at: m.created_at.to_rfc3339(),
            last_used_at: m.last_used_at.map(|t| t.to_rfc3339()),
            revoked: m.revoked_at.is_some(),
            revoked_at: m.revoked_at.map(|t| t.to_rfc3339()),
        }
    }
}

/// The audit actions of this module, named after the partner console's
/// `partner.publish_token.*` pair for the same two events.
pub const MINTED: &str = "admin.publish_token.minted";
pub const REVOKED: &str = "admin.publish_token.revoked";

/// What a new-format API token is told when it asks to mint a publish token.
pub struct MintPublishToken;
impl SessionAction for MintPublishToken {
    const REFUSAL: &'static str = "minting an app publish token requires a browser session";
    const CODE: Option<&'static str> = Some(SESSION_REQUIRED);
}

/// A 500 for a failed step, logged with what the step was.
fn internal<E: std::fmt::Display>(step: &'static str) -> impl FnOnce(E) -> StatusCode {
    move |e| {
        tracing::error!("app publish token: {step} failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

/// The audit row of a mint or a revoke: the publish token is the target, named
/// by id, label and non-secret display prefix. Never the plaintext or the hash.
fn token_event(
    actor: &RequestActor,
    action: &'static str,
    token: &app_publish_tokens::Model,
) -> AuditEntry {
    AuditEntry::for_request(actor, action)
        .target(
            "app_publish_token",
            token.id.to_string(),
            token.name.clone(),
        )
        .metadata(json!({
            "token_prefix": token.token_prefix,
            "minted_by": token.created_by,
        }))
}

pub async fn create_token(
    _: crate::server::api::custom_apps_agent_refusal::RefuseSandboxAgent,
    _: SessionOrLegacyKey<MintPublishToken>,
    actor: RequestActor,
    body: Option<Json<CreateTokenBody>>,
) -> Result<Json<CreateTokenResponse>, StatusCode> {
    let body = body
        .map(|Json(b)| b)
        .unwrap_or(CreateTokenBody { name: None });
    let name = body
        .name
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("app-publish-token {}", Utc::now().format("%Y-%m-%d")));

    let db = establish_connection()
        .await
        .map_err(internal("create_token DB connect"))?;

    let generated = generate_token();
    let now = Utc::now().fixed_offset();
    let id = Uuid::new_v4();

    // The token and the row that says who minted it commit together.
    let txn = db.begin().await.map_err(internal("create_token begin"))?;
    let token = app_publish_tokens::ActiveModel {
        id: ActiveValue::Set(id),
        name: ActiveValue::Set(name.clone()),
        token_hash: ActiveValue::Set(generated.token_hash),
        token_prefix: ActiveValue::Set(generated.token_prefix.clone()),
        created_by: ActiveValue::Set(Some(actor.id)),
        created_at: ActiveValue::Set(now),
        last_used_at: ActiveValue::Set(None),
        revoked_at: ActiveValue::Set(None),
        // Staff-minted tokens stay app-unscoped and non-expiring — the existing
        // Oxy-engineer CI flow. App-scoped fallback tokens (design §7) are minted
        // elsewhere with both set.
        app_id: ActiveValue::Set(None),
        expires_at: ActiveValue::Set(None),
    }
    .insert(&txn)
    .await
    .map_err(internal("create_token insert"))?;
    audit::record_in_txn(&txn, token_event(&actor, MINTED, &token))
        .await
        .map_err(internal("create_token audit"))?;
    txn.commit()
        .await
        .map_err(internal("create_token commit"))?;

    Ok(Json(CreateTokenResponse {
        id,
        token: generated.plaintext,
        name,
        token_prefix: generated.token_prefix,
        created_at: now.to_rfc3339(),
    }))
}

/// Does this caller get the shared cross-admin view of every staff token?
///
/// `Cap::OperatePlatform` — the "operates Oxy's own machinery" capability. Held by
/// Global Admins and owners, not by App Operators, which is exactly the line: fleet
/// operators audit the token estate; an app publisher manages their own credential.
async fn sees_all_tokens(
    db: &sea_orm::DatabaseConnection,
    actor: &oxy_auth::types::AuthenticatedUser,
) -> bool {
    let caller = crate::server::authz::Caller::from_user(actor);
    crate::server::authz::globals::platform_holds(db, &caller, oxy_authz::Cap::OperatePlatform)
        .await
}

pub async fn list_tokens(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
) -> Result<Json<Vec<TokenResponse>>, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("list_tokens DB connect failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let mut query = AppPublishTokens::find().order_by_desc(app_publish_tokens::Column::CreatedAt);
    if !sees_all_tokens(&db, &actor).await {
        query = query.filter(app_publish_tokens::Column::CreatedBy.eq(actor.id));
    }
    let rows = query.all(&db).await.map_err(|e| {
        tracing::error!("list_tokens query failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::ActiveValue;

    fn sample_model(revoked: bool) -> app_publish_tokens::Model {
        let now = Utc::now().fixed_offset();
        app_publish_tokens::Model {
            id: Uuid::new_v4(),
            name: "ci-publish".to_string(),
            token_hash: "deadbeef".repeat(8),
            token_prefix: "oxypublish_ab12cd34".to_string(),
            created_by: Some(Uuid::new_v4()),
            created_at: now,
            last_used_at: None,
            revoked_at: revoked.then_some(now),
            app_id: None,
            expires_at: None,
        }
    }

    #[test]
    fn list_view_never_leaks_secret_material() {
        let model = sample_model(false);
        let secret_hash = model.token_hash.clone();
        let resp = TokenResponse::from(model);
        let json = serde_json::to_string(&resp).unwrap();
        // The metadata view must expose neither the hash nor any plaintext.
        assert!(!json.contains(&secret_hash));
        assert!(!json.contains("token_hash"));
        assert!(json.contains("token_prefix"));
        assert!(!resp.revoked);
    }

    #[test]
    fn the_audit_row_names_the_token_and_carries_no_secret() {
        let model = sample_model(false);
        let actor = RequestActor::session(oxy_auth::types::AuthenticatedUser {
            id: Uuid::new_v4(),
            email: Some("staff@oxy.tech".to_string()),
            name: "Staff".to_string(),
            picture: None,
            status: entity::users::UserStatus::Active,
            credential: None,
        });
        for action in [MINTED, REVOKED] {
            let entry = token_event(&actor, action, &model);
            assert_eq!(entry.action, action);
            assert_eq!(entry.actor_user_id, Some(actor.id));
            assert_eq!(entry.target_type.as_deref(), Some("app_publish_token"));
            assert_eq!(entry.target_id, Some(model.id.to_string()));
            assert_eq!(entry.org_id, None, "a platform-level event");
            let written = entry.effective_metadata();
            assert_eq!(written["token_prefix"], json!(model.token_prefix));
            assert_eq!(written["minted_by"], json!(model.created_by));
            assert!(!written.to_string().contains(&model.token_hash));
        }
    }

    #[test]
    fn revoked_flag_reflects_revoked_at() {
        assert!(TokenResponse::from(sample_model(true)).revoked);
        assert!(!TokenResponse::from(sample_model(false)).revoked);
    }

    #[test]
    fn create_persists_hash_and_prefix_not_plaintext() {
        // Simulate the persistence step the handler performs: the ActiveModel
        // must carry the hash + prefix but never the plaintext.
        let generated = generate_token();
        let model = app_publish_tokens::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            name: ActiveValue::Set("t".to_string()),
            token_hash: ActiveValue::Set(generated.token_hash.clone()),
            token_prefix: ActiveValue::Set(generated.token_prefix.clone()),
            created_by: ActiveValue::Set(Some(Uuid::new_v4())),
            created_at: ActiveValue::Set(Utc::now().fixed_offset()),
            last_used_at: ActiveValue::Set(None),
            revoked_at: ActiveValue::Set(None),
            app_id: ActiveValue::Set(None),
            expires_at: ActiveValue::Set(None),
        };
        let ActiveValue::Set(stored_hash) = model.token_hash else {
            panic!("hash not set");
        };
        assert_eq!(stored_hash, generated.token_hash);
        assert_ne!(stored_hash, generated.plaintext);
    }
}

pub async fn revoke_token(
    _: crate::server::api::custom_apps_agent_refusal::RefuseSandboxAgent,
    actor: RequestActor,
    Path(id): Path<Uuid>,
) -> Result<Json<TokenResponse>, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(internal("revoke_token DB connect"))?;

    let token = AppPublishTokens::find_by_id(id)
        .one(&db)
        .await
        .map_err(internal("revoke_token lookup"))?
        .ok_or(StatusCode::NOT_FOUND)?;

    // Revoking someone else's token is a fleet-operator action, not an app-publishing
    // one — see the module docs. 404 rather than 403, so a bounded caller can't confirm
    // another operator's token exists by probing ids.
    if token.created_by != Some(actor.id) && !sees_all_tokens(&db, &actor).await {
        return Err(StatusCode::NOT_FOUND);
    }

    // Idempotent: re-revoking an already-revoked token is a no-op success, and
    // changes nothing to record.
    if token.revoked_at.is_some() {
        return Ok(Json(token.into()));
    }

    // The revoke and the row that says who revoked whose token commit together.
    let txn = db.begin().await.map_err(internal("revoke_token begin"))?;
    let mut active: app_publish_tokens::ActiveModel = token.into();
    active.revoked_at = ActiveValue::Set(Some(Utc::now().fixed_offset()));
    let updated = active
        .update(&txn)
        .await
        .map_err(internal("revoke_token update"))?;
    audit::record_in_txn(&txn, token_event(&actor, REVOKED, &updated))
        .await
        .map_err(internal("revoke_token audit"))?;
    txn.commit()
        .await
        .map_err(internal("revoke_token commit"))?;

    Ok(Json(updated.into()))
}
