//! The legacy workspace API-keys endpoint, `/api/{workspace_id}/api-keys`.
//!
//! This is the home of **legacy API keys**, which are separate from API tokens
//! (decided 2026-10-05). Its semantics are unchanged by the API-tokens work
//! (design §3.5): a key is the caller's, reaches everything the caller reaches,
//! and the list reads `api_keys`. Create mints a legacy key — `oxy_<32 hex>`,
//! never an `oxy_pat_` token. What changed underneath: every key is mirrored,
//! hashed, into `api_tokens` so one lookup, audit stamp and usage rollup cover
//! it; every write lands in both tables ([`lifecycle`]); and keys gain Extend
//! ([`extend`]) and Activity ([`activity`]). No token route returns a legacy key.

pub mod activity;
pub mod extend;
mod lifecycle;

pub use activity::get_api_key_activity;
pub use extend::extend_api_key;

use crate::server::api::middlewares::role_guards::WorkspaceAdmin;
use crate::server::service::api_key::{ApiKeyService, CreateApiKeyParams, CreateApiKeyResponse};
use axum::{
    Extension,
    extract::{self, Path},
    http::StatusCode,
    response::IntoResponse,
};
use entity::api_keys::Model as ApiKeyModel;
use garde::Validate;
use oxy::database::client::establish_connection;
use oxy_app_core::audit::RequestActor;
use oxy_auth::extractor::{AuthenticatedUserExtractor, SessionAction, SessionOrLegacyKey};
use oxy_shared::errors::OxyError;
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;
use uuid::Uuid;

use lifecycle::Actor;

/// Create and revoke accepted any key before tokens existed, and a legacy key
/// keeps that (design §3.5). A new-format token is refused: a token cannot
/// mint or revoke a token.
pub struct CreateKey;
impl SessionAction for CreateKey {
    const REFUSAL: &'static str = "creating an API key requires a browser session";
}

pub struct RevokeKey;
impl SessionAction for RevokeKey {
    const REFUSAL: &'static str = "revoking an API key requires a browser session";
}

// Validation functions for expires_at fields
fn validate_expires_at(
    value: &Option<chrono::DateTime<chrono::Utc>>,
    _context: &(),
) -> garde::Result {
    if let Some(expires_at) = value {
        let now = chrono::Utc::now();
        if *expires_at <= now {
            return Err(garde::Error::new("expires_at must be in the future"));
        }
    }
    Ok(())
}

// Helper function to mask API keys for safe display
fn mask_api_key(key: &str) -> String {
    if key.len() <= 8 {
        // If key is too short, just show asterisks
        "*".repeat(key.len())
    } else {
        // Show first 4 and last 4 characters, mask the middle
        let start = &key[..4];
        let end = &key[key.len() - 4..];
        let middle_len = key.len() - 8;
        format!("{}{}...{}", start, "*".repeat(middle_len.min(8)), end)
    }
}

#[derive(Serialize, ToSchema)]
pub struct ApiKeyResponse {
    pub id: String,
    pub name: String,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub is_active: bool,
    #[schema(example = "sk_1234****...5678")]
    pub masked_key: Option<String>, // Only shown for newly created keys or when explicitly requested
}

impl ApiKeyResponse {
    // Create a new response with a masked key (for newly created keys)
    pub fn with_masked_key(mut self, key: &str) -> Self {
        self.masked_key = Some(mask_api_key(key));
        self
    }

    // Create a response without any key information (for listing existing keys)
    pub fn without_key(mut self) -> Self {
        self.masked_key = None;
        self
    }
}

#[derive(Serialize, ToSchema)]
pub struct ApiKeyListResponse {
    pub api_keys: Vec<ApiKeyResponse>,
    pub total: usize,
}

#[derive(Deserialize, ToSchema, Validate)]
pub struct CreateApiKeyRequest {
    #[garde(length(min = 1, max = 100))]
    #[schema(example = "My Production API Key")]
    pub name: String,

    #[garde(custom(validate_expires_at))]
    #[schema(example = "2025-12-31T23:59:59Z")]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Serialize, ToSchema)]
pub struct CreateApiKeyResponseDto {
    pub id: String,
    #[schema(example = "sk_1234567890abcdef...")]
    pub key: String, // Full key only shown on creation
    #[schema(example = "sk_1234****...cdef")]
    pub masked_key: String, // Masked version for safer display
    pub name: String,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<CreateApiKeyResponse> for CreateApiKeyResponseDto {
    fn from(response: CreateApiKeyResponse) -> Self {
        let masked = mask_api_key(&response.key);

        Self {
            id: response.id.to_string(),
            key: response.key.clone(),
            masked_key: masked,
            name: response.name,
            expires_at: response.expires_at,
            created_at: response.created_at,
        }
    }
}

impl From<ApiKeyModel> for ApiKeyResponse {
    fn from(model: ApiKeyModel) -> Self {
        Self {
            id: model.id.to_string(),
            name: model.name,
            expires_at: model.expires_at.map(|dt| dt.into()),
            last_used_at: model.last_used_at.map(|dt| dt.into()),
            created_at: model.created_at.into(),
            is_active: model.is_active,
            masked_key: None, // Never expose key data from stored models
        }
    }
}

/// Create a new API key
#[utoipa::path(
    post,
    path = "/{workspace_id}/api-keys",
    request_body = CreateApiKeyRequest,
    responses(
        (status = 201, description = "API key created successfully", body = CreateApiKeyResponseDto),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not a workspace admin, or called with a new-format token"),
        (status = 422, description = "Validation error"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("ApiKey" = [])
    ),
    tag = "API Keys",
    params(
        ("workspace_id" = Uuid, Path, description = "Workspace UUID")
    ),
)]
pub async fn create_api_key(
    _: SessionOrLegacyKey<CreateKey>,
    _: WorkspaceAdmin,
    user: RequestActor,
    workspace: Option<Extension<entity::workspaces::Model>>,
    Path(workspace_id): Path<Uuid>,
    extract::Json(request): extract::Json<CreateApiKeyRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    if let Err(validation_errors) = request.validate() {
        tracing::warn!("API key creation validation failed: {}", validation_errors);
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            extract::Json(json!({
                "error": "Validation failed",
                "details": validation_errors.to_string()
            })),
        )
            .into_response());
    }

    let db = establish_connection().await?;
    let actor = Actor {
        request: &user,
        workspace_id,
        org_id: lifecycle::org_of(&db, workspace, workspace_id).await?,
    };
    let create_request = CreateApiKeyParams {
        user_id: user.id,
        name: request.name,
        expires_at: request.expires_at,
        project_id: workspace_id,
    };

    match lifecycle::create(&db, &actor, create_request).await {
        Ok(response) => {
            let dto: CreateApiKeyResponseDto = response.into();
            Ok((StatusCode::CREATED, extract::Json(dto)).into_response())
        }
        Err(OxyError::ValidationError(msg)) => {
            tracing::error!("API key service validation error: {}", msg);
            Ok((
                StatusCode::BAD_REQUEST,
                extract::Json(json!({
                    "error": msg
                })),
            )
                .into_response())
        }
        Err(e) => {
            tracing::error!("Failed to create API key: {}", e);
            Ok((
                StatusCode::INTERNAL_SERVER_ERROR,
                extract::Json(json!({
                    "error": "Internal server error"
                })),
            )
                .into_response())
        }
    }
}

/// List user's API keys
#[utoipa::path(
    get,
    path = "/{workspace_id}/api-keys",
    responses(
        (status = 200, description = "List of API keys", body = ApiKeyListResponse),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("ApiKey" = [])
    ),
    tag = "API Keys",
    params(
        ("workspace_id" = Uuid, Path, description = "Workspace UUID")
    ),
)]
pub async fn list_api_keys(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
) -> Result<impl IntoResponse, StatusCode> {
    let db = establish_connection().await?;

    match ApiKeyService::list_user_api_keys(&db, user.id).await {
        Ok(api_keys) => {
            let api_key_responses: Vec<ApiKeyResponse> = api_keys
                .into_iter()
                .map(|key| ApiKeyResponse::from(key).without_key())
                .collect();

            let response = ApiKeyListResponse {
                total: api_key_responses.len(),
                api_keys: api_key_responses,
            };

            Ok(extract::Json(response))
        }
        Err(e) => {
            tracing::error!("Failed to list API keys: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Get specific API key info
#[utoipa::path(
    get,
    path = "/{workspace_id}/api-keys/{id}",
    responses(
        (status = 200, description = "API key details", body = ApiKeyResponse),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "API key not found"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("ApiKey" = [])
    ),
    tag = "API Keys",
    params(
        ("workspace_id" = Uuid, Path, description = "Workspace UUID"),
        ("id" = String, Path, description = "API key ID")
    ),
)]
pub async fn get_api_key(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    // Both segments: the route is nested under `/{workspace_id}`, and a bare
    // `Path<String>` against two parameters is a 500, not the key.
    Path((_workspace_id, id)): Path<(Uuid, String)>,
) -> Result<impl IntoResponse, StatusCode> {
    let key_id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let db = establish_connection().await?;

    // Get all user's API keys and find the requested one
    match ApiKeyService::list_user_api_keys(&db, user.id).await {
        Ok(api_keys) => {
            if let Some(api_key) = api_keys.into_iter().find(|k| k.id == key_id) {
                let response = ApiKeyResponse::from(api_key).without_key();
                Ok(extract::Json(response))
            } else {
                Err(StatusCode::NOT_FOUND)
            }
        }
        Err(e) => {
            tracing::error!("Failed to get API key: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Revoke (delete) an API key
#[utoipa::path(
    delete,
    path = "/{workspace_id}/api-keys/{id}",
    responses(
        (status = 204, description = "API key revoked successfully"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not a workspace admin, or called with a new-format token"),
        (status = 404, description = "API key not found"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("ApiKey" = [])
    ),
    tag = "API Keys",
    params(
        ("workspace_id" = Uuid, Path, description = "Workspace UUID"),
        ("id" = String, Path, description = "API key ID")
    ),
)]
pub async fn delete_api_key(
    _: SessionOrLegacyKey<RevokeKey>,
    _: WorkspaceAdmin,
    user: RequestActor,
    workspace: Option<Extension<entity::workspaces::Model>>,
    Path((workspace_id, id)): Path<(Uuid, String)>,
) -> Result<impl IntoResponse, StatusCode> {
    let key_id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let db = establish_connection().await?;
    let actor = Actor {
        request: &user,
        workspace_id,
        org_id: lifecycle::org_of(&db, workspace, workspace_id).await?,
    };

    match lifecycle::revoke(&db, &actor, key_id).await {
        Ok(_) => Ok(StatusCode::NO_CONTENT),
        Err(OxyError::ValidationError(_)) => Err(StatusCode::NOT_FOUND),
        Err(e) => {
            tracing::error!("Failed to revoke API key: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}
