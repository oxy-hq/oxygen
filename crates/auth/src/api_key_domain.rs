//! API key domain: the legacy `/api/{workspace_id}/api-keys` endpoint's writes.
//!
//! Release N of the API-tokens design (§7). Every write lands in **both**
//! tables, so either release can read what the other wrote:
//!
//! - **create** mints an `oxy_pat_` token, stores its SHA-256 in `api_tokens`
//!   and — for one release only — its plaintext in `api_keys`, which is what a
//!   pod one release back validates by;
//! - **revoke** marks `api_keys.is_active = false` and sets
//!   `api_tokens.revoked_at`;
//! - **extend** moves `expires_at` in both.
//!
//! Each takes any `ConnectionTrait` so the caller can run it, and its audit
//! row, in one transaction.

use chrono::{DateTime, Utc};
use entity::prelude::{ApiKeys, ApiTokens};
use entity::{api_keys, api_tokens};
use oxy_shared::errors::OxyError;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    EntityTrait, QueryFilter, Set, Statement,
};
use uuid::Uuid;

use crate::api_key_extend::ExtendTo;
use crate::token::credential::{StoredKind, source};
use crate::token::format::{display_prefix, generate_legacy_key};
use crate::token::store::{ensure_legacy_row, ensure_user_legacy_rows, find_active_legacy_key};

#[derive(Debug, Clone)]
pub struct ApiKeyConfig {
    pub require_user_active: bool,
    pub allow_multiple_keys: bool,
}

impl Default for ApiKeyConfig {
    fn default() -> Self {
        Self {
            require_user_active: true,
            allow_multiple_keys: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ValidatedApiKey {
    pub id: uuid::Uuid,
    pub key: String,
    pub user_id: uuid::Uuid,
}

#[derive(Debug, Clone)]
pub struct CreateApiKeyParams {
    pub user_id: uuid::Uuid,
    pub name: String,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub project_id: uuid::Uuid,
}

#[derive(Debug, Clone)]
pub struct CreateApiKeyResponse {
    /// The `api_keys` id — and the token id, which is the same uuid.
    pub id: uuid::Uuid,
    /// The plaintext, returned exactly once.
    pub key: String,
    pub name: String,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub kind: StoredKind,
    pub display_prefix: String,
}

/// What a revoke did, for the caller's audit row.
#[derive(Debug, Clone)]
pub struct RevokedApiKey {
    pub token_id: Uuid,
    pub name: String,
    /// False when the key was already revoked: nothing changed.
    pub newly_revoked: bool,
    pub mirror: Option<api_tokens::Model>,
}

/// What an extension did.
#[derive(Debug, Clone)]
pub struct ExtendedApiKey {
    pub api_key: api_keys::Model,
    pub previous_expires_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub mirror: Option<api_tokens::Model>,
}

/// Why an extension was refused.
#[derive(Debug, thiserror::Error)]
pub enum ExtendError {
    /// Unknown, or not the caller's: indistinguishable on purpose.
    #[error("API key not found")]
    NotFound,
    #[error("API key is revoked")]
    Revoked,
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Db(#[from] OxyError),
}

fn db_err(what: &'static str) -> impl FnOnce(sea_orm::DbErr) -> OxyError {
    move |e| OxyError::DBError(format!("{what}: {e}"))
}

const REVOKE_MIRROR_SQL: &str = "UPDATE api_tokens SET revoked_at = now(), revoked_by = $2::uuid, \
     revoke_reason = 'owner' WHERE legacy_api_key_id = $1::uuid AND revoked_at IS NULL";
/// The mirror takes the new expiry, and a new expiry gets a new 7-day notice
/// (`token::hygiene`).
const EXTEND_MIRROR_SQL: &str = "UPDATE api_tokens SET expires_at = $2::timestamptz, \
     expiry_notified_at = NULL, expired_reason = NULL, renewed_at = now() \
     WHERE legacy_api_key_id = $1::uuid";

pub struct ApiKeyService;

impl ApiKeyService {
    /// Today's plaintext lookup in `api_keys` (active, unexpired). Request
    /// authentication goes through `token::authenticate_request`, which uses
    /// this only as the fallback for a key with no `api_tokens` row yet.
    pub async fn validate_api_key(
        db: &DatabaseConnection,
        key: &str,
        _config: &ApiKeyConfig,
    ) -> Result<ValidatedApiKey, OxyError> {
        let api_key = find_active_legacy_key(db, key)
            .await?
            .ok_or_else(|| OxyError::AuthenticationError("Invalid API key".to_string()))?;
        Ok(ValidatedApiKey {
            id: api_key.id,
            key: key.to_string(),
            user_id: api_key.user_id,
        })
    }

    /// Mint a **legacy API key** — `oxy_<32 hex>`, the shape this endpoint has
    /// always returned — and mirror it into `api_tokens` under the same id.
    /// A legacy key is never an `oxy_pat_` token: tokens are minted only by
    /// the token routes. Run inside a transaction.
    pub async fn create_api_key<C: ConnectionTrait>(
        db: &C,
        params: CreateApiKeyParams,
        _config: &ApiKeyConfig,
    ) -> Result<CreateApiKeyResponse, OxyError> {
        let key = generate_legacy_key();
        let id = Uuid::new_v4();
        let now = Utc::now();
        let expires_at = params.expires_at.map(|dt| dt.fixed_offset());

        api_keys::ActiveModel {
            id: Set(id),
            user_id: Set(params.user_id),
            // The plaintext, as a legacy key has always been stored: a pod one
            // release back validates it from this column mid-rollout and after
            // a revert. Clearing it is Phase 6, a later release (design §7).
            key_hash: Set(key.clone()),
            name: Set(params.name.clone()),
            expires_at: Set(expires_at),
            created_at: Set(now.into()),
            updated_at: Set(now.into()),
            is_active: Set(true),
            project_id: Set(params.project_id),
            last_used_at: Set(None),
            app_id: Set(None),
        }
        .insert(db)
        .await
        .map_err(db_err("Failed to create API key"))?;

        // The hashed mirror row (`kind = legacy_key`), written the same way
        // the backfill and a key's first use write it, so the one lookup,
        // audit stamp and usage rollup cover this key too.
        ensure_legacy_row(db, id, source::LEGACY_ENDPOINT).await?;

        Ok(CreateApiKeyResponse {
            id,
            display_prefix: display_prefix(&key),
            key,
            name: params.name,
            expires_at: params.expires_at,
            created_at: now,
            kind: StoredKind::LegacyKey,
        })
    }

    pub async fn list_user_api_keys(
        db: &DatabaseConnection,
        user_id: uuid::Uuid,
    ) -> Result<Vec<::entity::api_keys::Model>, OxyError> {
        // Best-effort: a failed mirror costs a not-yet-used key its Activity
        // and its row in the org inventory until first use, never this list.
        if let Err(e) = ensure_user_legacy_rows(db, user_id).await {
            tracing::warn!(user = %user_id, error = %e, "could not mirror legacy keys before listing");
        }
        ApiKeys::find()
            .filter(api_keys::Column::UserId.eq(user_id))
            .filter(api_keys::Column::IsActive.eq(true))
            .all(db)
            .await
            .map_err(|e| OxyError::DBError(format!("Database error: {}", e)))
    }

    /// The caller's own key, or `ValidationError` — not found and not theirs
    /// read the same.
    async fn owned_key<C: ConnectionTrait>(
        db: &C,
        key_id: Uuid,
        user_id: Uuid,
    ) -> Result<api_keys::Model, OxyError> {
        Self::find_owned_key(db, key_id, user_id)
            .await?
            .ok_or_else(|| OxyError::ValidationError("API key not found".to_string()))
    }

    /// The caller's own key, revoked or not. `None` when it does not exist or
    /// belongs to someone else — the two are indistinguishable on purpose.
    pub async fn find_owned_key<C: ConnectionTrait>(
        db: &C,
        key_id: Uuid,
        user_id: Uuid,
    ) -> Result<Option<api_keys::Model>, OxyError> {
        Ok(ApiKeys::find_by_id(key_id)
            .one(db)
            .await
            .map_err(db_err("Database error"))?
            .filter(|k| k.user_id == user_id))
    }

    /// The `api_tokens` row mirroring an `api_keys` row, when there is one.
    pub async fn mirror_of<C: ConnectionTrait>(
        db: &C,
        key_id: Uuid,
    ) -> Result<Option<api_tokens::Model>, OxyError> {
        ApiTokens::find()
            .filter(api_tokens::Column::LegacyApiKeyId.eq(key_id))
            .one(db)
            .await
            .map_err(db_err("api token lookup"))
    }

    /// Revoke the caller's key in both tables. Idempotent: revoking a revoked
    /// key changes nothing and reports `newly_revoked: false`. Run inside a
    /// transaction; invalidate the credential cache after it commits.
    pub async fn revoke_api_key<C: ConnectionTrait>(
        db: &C,
        key_id: Uuid,
        user_id: Uuid,
    ) -> Result<RevokedApiKey, OxyError> {
        let api_key = Self::owned_key(db, key_id, user_id).await?;
        let newly_revoked = api_key.is_active;
        // Mirror first, while the key still reads as active, so the mirror is
        // revoked below with who and why rather than born revoked.
        ensure_legacy_row(db, key_id, source::LEGACY_LAZY).await?;

        let name = api_key.name.clone();
        let mut active: api_keys::ActiveModel = api_key.into();
        active.is_active = Set(false);
        active.updated_at = Set(Utc::now().into());
        active
            .update(db)
            .await
            .map_err(db_err("Failed to revoke API key"))?;

        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            REVOKE_MIRROR_SQL,
            [key_id.into(), user_id.into()],
        ))
        .await
        .map_err(db_err("Failed to revoke API token"))?;

        Ok(RevokedApiKey {
            token_id: key_id,
            name,
            newly_revoked,
            mirror: Self::mirror_of(db, key_id).await?,
        })
    }

    /// Move the caller's key's expiry in both tables (design §3.6). Works on an
    /// expired key — that is how a lapsed key is revived where it is deployed —
    /// but never on a revoked one. Run inside a transaction.
    pub async fn extend_api_key<C: ConnectionTrait>(
        db: &C,
        key_id: Uuid,
        user_id: Uuid,
        target: ExtendTo,
    ) -> Result<ExtendedApiKey, ExtendError> {
        let api_key = match Self::owned_key(db, key_id, user_id).await {
            Ok(k) => k,
            Err(OxyError::ValidationError(_)) => return Err(ExtendError::NotFound),
            Err(e) => return Err(e.into()),
        };
        ensure_legacy_row(db, key_id, source::LEGACY_LAZY).await?;
        let mirror = Self::mirror_of(db, key_id).await?;
        if !api_key.is_active || mirror.as_ref().is_some_and(|m| m.revoked_at.is_some()) {
            return Err(ExtendError::Revoked);
        }

        let now = Utc::now();
        let previous = api_key.expires_at.map(DateTime::<Utc>::from);
        let expires_at = target
            .resolve(previous, now)
            .map_err(ExtendError::Invalid)?;
        let stored = expires_at.map(|t| t.fixed_offset());

        let mut active: api_keys::ActiveModel = api_key.into();
        active.expires_at = Set(stored);
        active.updated_at = Set(now.into());
        let api_key = active
            .update(db)
            .await
            .map_err(db_err("Failed to extend API key"))?;

        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            EXTEND_MIRROR_SQL,
            [key_id.into(), stored.into()],
        ))
        .await
        .map_err(db_err("Failed to extend API token"))?;

        Ok(ExtendedApiKey {
            api_key,
            previous_expires_at: previous,
            expires_at,
            mirror: Self::mirror_of(db, key_id).await?,
        })
    }
}
