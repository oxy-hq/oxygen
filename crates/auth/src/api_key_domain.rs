//! API Key domain types and service

use oxy_shared::errors::OxyError;
use sea_orm::DatabaseConnection;

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
    pub id: uuid::Uuid,
    pub key: String,
    pub name: String,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub struct ApiKeyService;

impl ApiKeyService {
    pub async fn validate_api_key(
        db: &DatabaseConnection,
        key: &str,
        _config: &ApiKeyConfig,
    ) -> Result<ValidatedApiKey, OxyError> {
        use ::entity::prelude::*;
        use sea_orm::*;

        let now = chrono::Utc::now().fixed_offset();

        let api_key = ApiKeys::find()
            .filter(::entity::api_keys::Column::KeyHash.eq(key))
            .filter(::entity::api_keys::Column::IsActive.eq(true))
            .filter(
                Condition::any()
                    .add(::entity::api_keys::Column::ExpiresAt.is_null())
                    .add(::entity::api_keys::Column::ExpiresAt.gt(now)),
            )
            .one(db)
            .await
            .map_err(|e| OxyError::DBError(format!("Database error: {}", e)))?
            .ok_or_else(|| OxyError::AuthenticationError("Invalid API key".to_string()))?;

        Ok(ValidatedApiKey {
            id: api_key.id,
            key: key.to_string(),
            user_id: api_key.user_id,
        })
    }

    pub async fn create_api_key(
        db: &DatabaseConnection,
        params: CreateApiKeyParams,
        _config: &ApiKeyConfig,
    ) -> Result<CreateApiKeyResponse, OxyError> {
        use sea_orm::*;

        let key = format!("oxy_{}", uuid::Uuid::new_v4().to_string().replace("-", ""));
        let now = chrono::Utc::now();

        let api_key = ::entity::api_keys::ActiveModel {
            id: Set(uuid::Uuid::new_v4()),
            user_id: Set(params.user_id),
            // TODO(hash-at-rest): store sha256(key) and have validate_api_key
            // hash the lookup key.
            key_hash: Set(key.clone()),
            name: Set(params.name.clone()),
            expires_at: Set(params.expires_at.map(|dt| dt.into())),
            created_at: Set(now.into()),
            updated_at: Set(now.into()),
            is_active: Set(true),
            project_id: Set(params.project_id),
            last_used_at: NotSet,
            // User-scoped keys aren't bound to a custom-app row. Nothing mints
            // an app-bound key any more (the v0/Vercel minter was removed);
            // the column stays for the rows that already carry one.
            app_id: NotSet,
        };

        let result = api_key
            .insert(db)
            .await
            .map_err(|e| OxyError::DBError(format!("Failed to create API key: {}", e)))?;

        Ok(CreateApiKeyResponse {
            id: result.id,
            key,
            name: result.name,
            expires_at: params.expires_at,
            created_at: now,
        })
    }

    pub async fn list_user_api_keys(
        db: &DatabaseConnection,
        user_id: uuid::Uuid,
    ) -> Result<Vec<::entity::api_keys::Model>, OxyError> {
        use ::entity::prelude::*;
        use sea_orm::*;

        let api_keys = ApiKeys::find()
            .filter(::entity::api_keys::Column::UserId.eq(user_id))
            .filter(::entity::api_keys::Column::IsActive.eq(true))
            .all(db)
            .await
            .map_err(|e| OxyError::DBError(format!("Database error: {}", e)))?;

        Ok(api_keys)
    }

    pub async fn revoke_api_key(
        db: &DatabaseConnection,
        key_id: uuid::Uuid,
        user_id: uuid::Uuid,
    ) -> Result<(), OxyError> {
        use ::entity::prelude::*;
        use sea_orm::*;

        // Find the key and verify ownership
        let api_key = ApiKeys::find_by_id(key_id)
            .one(db)
            .await
            .map_err(|e| OxyError::DBError(format!("Database error: {}", e)))?
            .ok_or_else(|| OxyError::ValidationError("API key not found".to_string()))?;

        if api_key.user_id != user_id {
            return Err(OxyError::ValidationError("API key not found".to_string()));
        }

        // Mark as inactive (soft delete)
        let mut api_key: ::entity::api_keys::ActiveModel = api_key.into();
        api_key.is_active = Set(false);
        api_key.updated_at = Set(chrono::Utc::now().into());

        api_key
            .update(db)
            .await
            .map_err(|e| OxyError::DBError(format!("Failed to revoke API key: {}", e)))?;

        Ok(())
    }
}
