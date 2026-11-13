//! Create, revoke and extend, each with its lifecycle audit row in the same
//! transaction (API-tokens design §3.7, layer 1).
//!
//! `record_in_txn`, not `record`: the key and its audit row commit or roll back
//! together, so a key never exists without the row saying who made it, and a
//! failed audit write fails the request instead of being dropped. The audit row
//! names the token by id and non-secret prefix — **never the token itself**.
//!
//! Scoped to the org of the workspace in the route (design §3.7: "legacy keys
//! get the same events, scoped to the org of the workspace in their route").

use axum::Extension;
use chrono::{DateTime, Utc};
use entity::prelude::Workspaces;
use entity::workspaces;
use oxy_app_core::audit::{self, AuditEntry, RequestActor, TOKEN_TARGET_TYPE};
use oxy_auth::{
    ApiKeyConfig, ApiKeyService, CreateApiKeyParams, CreateApiKeyResponse, ExtendError, ExtendTo,
    ExtendedApiKey, RevokedApiKey,
};
use oxy_shared::errors::OxyError;
use sea_orm::{DatabaseConnection, EntityTrait, TransactionTrait};
use serde_json::{Value, json};
use uuid::Uuid;

/// Who acted, and where. Built once per request by the handler.
pub(super) struct Actor<'a> {
    /// The user, plus the legacy key that made the call when it was not a
    /// session (create and revoke still accept one).
    pub request: &'a RequestActor,
    pub workspace_id: Uuid,
    pub org_id: Option<Uuid>,
}

/// The org of the route's workspace: from the row `workspace_middleware`
/// attached, else looked up. `None` only for a workspace with no org, which
/// the middleware refuses anyway; the event is then recorded unchained.
pub(super) async fn org_of(
    db: &DatabaseConnection,
    workspace: Option<Extension<workspaces::Model>>,
    workspace_id: Uuid,
) -> Result<Option<Uuid>, OxyError> {
    if let Some(Extension(row)) = workspace {
        return Ok(row.org_id);
    }
    Ok(Workspaces::find_by_id(workspace_id)
        .one(db)
        .await
        .map_err(|e| OxyError::DBError(format!("workspace lookup: {e}")))?
        .and_then(|w| w.org_id))
}

pub(super) async fn create(
    db: &DatabaseConnection,
    actor: &Actor<'_>,
    params: CreateApiKeyParams,
) -> Result<CreateApiKeyResponse, OxyError> {
    let txn = db.begin().await.map_err(db_err)?;
    let created = ApiKeyService::create_api_key(&txn, params, &ApiKeyConfig::default()).await?;
    audit::record_in_txn(&txn, created_entry(actor, &created))
        .await
        .map_err(db_err)?;
    txn.commit().await.map_err(db_err)?;
    Ok(created)
}

pub(super) async fn revoke(
    db: &DatabaseConnection,
    actor: &Actor<'_>,
    key_id: Uuid,
) -> Result<RevokedApiKey, OxyError> {
    let txn = db.begin().await.map_err(db_err)?;
    let revoked = ApiKeyService::revoke_api_key(&txn, key_id, actor.request.id).await?;
    // Revoking a revoked key changes nothing, so it records nothing.
    if revoked.newly_revoked {
        audit::record_in_txn(&txn, revoked_entry(actor, &revoked))
            .await
            .map_err(db_err)?;
    }
    txn.commit().await.map_err(db_err)?;
    oxy_auth::token::cache::invalidate_token(revoked.token_id);
    Ok(revoked)
}

pub(super) async fn extend(
    db: &DatabaseConnection,
    actor: &Actor<'_>,
    key_id: Uuid,
    target: ExtendTo,
) -> Result<ExtendedApiKey, ExtendError> {
    let txn = db.begin().await.map_err(db_err)?;
    let extended = ApiKeyService::extend_api_key(&txn, key_id, actor.request.id, target).await?;
    audit::record_in_txn(&txn, extended_entry(actor, &extended))
        .await
        .map_err(db_err)?;
    txn.commit().await.map_err(db_err)?;
    oxy_auth::token::cache::invalidate_token(key_id);
    Ok(extended)
}

fn db_err(e: sea_orm::DbErr) -> OxyError {
    OxyError::DBError(format!("api key lifecycle: {e}"))
}

/// The common shape: actor, scope, and the token as the target.
fn entry(actor: &Actor<'_>, action: &'static str, token_id: Uuid, name: &str) -> AuditEntry {
    let mut e = AuditEntry::for_request(actor.request, action)
        .workspace(actor.workspace_id)
        .target(TOKEN_TARGET_TYPE, token_id.to_string(), name);
    if let Some(org_id) = actor.org_id {
        e = e.org(org_id);
    }
    e
}

/// Metadata every lifecycle row carries about the token it is **about**: ids
/// and the non-secret prefix only.
///
/// When a legacy key (not a session) performs the action, `for_request` stamps
/// that key over `token_id` / `token_kind` / `display_prefix` — the stamp always
/// names the credential that acted. The token acted on is still the row's
/// target and its `api_key_id`.
fn metadata(kind: &str, display_prefix: &str, extra: Value) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("token_kind".into(), json!(kind));
    m.insert("display_prefix".into(), json!(display_prefix));
    if let Value::Object(extra) = extra {
        m.extend(extra);
    }
    Value::Object(m)
}

pub(super) fn created_entry(actor: &Actor<'_>, created: &CreateApiKeyResponse) -> AuditEntry {
    entry(actor, "token.created", created.id, &created.name).metadata(metadata(
        created.kind.as_str(),
        &created.display_prefix,
        json!({
            "token_id": created.id,
            "api_key_id": created.id,
            "source": oxy_auth::token::credential::source::LEGACY_ENDPOINT,
            "expires_at": rfc3339(created.expires_at),
            "all_access": true,
            "platform": true,
            "partner": true,
        }),
    ))
}

pub(super) fn revoked_entry(actor: &Actor<'_>, revoked: &RevokedApiKey) -> AuditEntry {
    let (kind, prefix) = mirror_labels(revoked.mirror.as_ref());
    entry(actor, "token.revoked", revoked.token_id, &revoked.name).metadata(metadata(
        &kind,
        &prefix,
        json!({
            "token_id": revoked.token_id,
            "api_key_id": revoked.token_id,
            "reason": "owner",
        }),
    ))
}

/// Old and new expiry go in `before`/`after` (the audit row's change pair) and
/// again in `metadata` as `old_expires_at`/`new_expires_at`, which is what the
/// activity endpoint surfaces for "extended from X to Y". RFC 3339, or null for
/// no expiry.
pub(super) fn extended_entry(actor: &Actor<'_>, extended: &ExtendedApiKey) -> AuditEntry {
    let (kind, prefix) = mirror_labels(extended.mirror.as_ref());
    let id = extended.api_key.id;
    let old = rfc3339(extended.previous_expires_at);
    let new = rfc3339(extended.expires_at);
    entry(actor, "token.extended", id, &extended.api_key.name)
        .change(
            json!({ "expires_at": old.clone() }),
            json!({ "expires_at": new.clone() }),
        )
        .metadata(metadata(
            &kind,
            &prefix,
            json!({
                "token_id": id,
                "api_key_id": id,
                "old_expires_at": old,
                "new_expires_at": new,
            }),
        ))
}

fn mirror_labels(mirror: Option<&entity::api_tokens::Model>) -> (String, String) {
    mirror.map_or_else(
        || ("legacy_key".to_string(), String::new()),
        |m| (m.kind.clone(), m.display_prefix.clone()),
    )
}

fn rfc3339(at: Option<DateTime<Utc>>) -> Value {
    at.map_or(Value::Null, |t| Value::String(t.to_rfc3339()))
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;
