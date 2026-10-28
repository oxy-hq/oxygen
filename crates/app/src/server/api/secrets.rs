use crate::server::api::middlewares::role_guards::WorkspaceAdmin;
use crate::server::service::secret_manager::{
    CreateSecretParams, SecretInfo, SecretManagerService, UpdateSecretParams,
};
use axum::{
    extract::{self, Path},
    http::StatusCode,
    response::IntoResponse,
};
use entity::users::Entity as Users;
use garde::Validate;
use oxy::database::client::establish_connection;
use oxy_app_core::audit::RequestActor;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_custom_apps::secret_audit::{ProjectSecretWrite, audit_if_app_secret};
use oxy_shared::errors::OxyError;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Serialize)]
pub struct SecretResponse {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub created_by: String,
    pub created_by_email: Option<String>,
    pub updated_by_email: Option<String>,
    pub is_active: bool,
}

impl From<SecretInfo> for SecretResponse {
    fn from(secret: SecretInfo) -> Self {
        Self {
            id: secret.id.to_string(),
            name: secret.name,
            description: secret.description,
            created_at: secret.created_at,
            updated_at: secret.updated_at,
            created_by: secret.created_by.to_string(),
            created_by_email: None, // populated by list_secrets after user lookup
            updated_by_email: None, // populated by list_secrets after user lookup
            is_active: secret.is_active,
        }
    }
}

/// Resolve a set of user UUIDs to their emails in one query.
pub async fn resolve_user_emails(
    db: &impl sea_orm::ConnectionTrait,
    ids: &[Uuid],
) -> HashMap<Uuid, String> {
    if ids.is_empty() {
        return HashMap::new();
    }
    match Users::find()
        .filter(entity::users::Column::Id.is_in(ids.to_vec()))
        .all(db)
        .await
    {
        Ok(users) => users
            .into_iter()
            .map(|u| (u.id, u.label().to_string()))
            .collect(),
        Err(e) => {
            tracing::warn!("Failed to resolve user emails: {}", e);
            HashMap::new()
        }
    }
}

#[derive(Serialize)]
pub struct SecretListResponse {
    pub secrets: Vec<SecretResponse>,
    pub total: usize,
}

#[derive(Deserialize, Validate, Serialize)]
pub struct CreateSecretRequest {
    #[garde(length(min = 1, max = 255))]
    pub name: String,
    #[garde(length(min = 1, max = 10000))]
    pub value: String,
    #[garde(length(min = 0, max = 1000))]
    pub description: Option<String>,
}

#[derive(Deserialize, Validate)]
pub struct BulkCreateSecretsRequest {
    #[garde(length(min = 1, max = 100), dive)]
    pub secrets: Vec<CreateSecretRequest>,
}

#[derive(Serialize)]
pub struct FailedSecret {
    pub secret: CreateSecretRequest,
    pub error: String,
}

#[derive(Serialize)]
pub struct BulkCreateSecretsResponse {
    pub created_secrets: Vec<SecretResponse>,
    pub failed_secrets: Vec<FailedSecret>,
}

#[derive(Deserialize, Validate)]
pub struct UpdateSecretRequest {
    #[garde(length(min = 1, max = 10000))]
    pub value: Option<String>,
    #[garde(length(min = 0, max = 1000))]
    pub description: Option<String>,
}

pub async fn create_secret(
    _: WorkspaceAdmin,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(workspace_id): Path<Uuid>,
    extract::Json(request): extract::Json<CreateSecretRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    if let Err(validation_errors) = request.validate() {
        tracing::warn!("Secret creation validation failed: {}", validation_errors);
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            axum::Json(json!({
                "error": "Validation failed",
                "details": validation_errors.to_string()
            })),
        )
            .into_response());
    }

    // Reject secrets whose names collide with infrastructure / auth env vars.
    // These vars are owned by the server operator and must not be overridable
    // through the per-workspace secrets panel.
    if is_auth_env_var(&request.name) {
        tracing::warn!(
            "Rejected secret creation: name '{}' is reserved for infrastructure config",
            request.name
        );
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            axum::Json(json!({
                "error": format!(
                    "'{}' is a reserved environment variable name used by Oxygen's infrastructure. \
                     Choose a different name for your secret.",
                    request.name
                )
            })),
        )
            .into_response());
    }

    let db = establish_connection().await.map_err(|e| {
        tracing::error!("Failed to establish database connection: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let secret_manager = SecretManagerService::new(workspace_id);

    let create_params = CreateSecretParams {
        name: request.name,
        value: request.value,
        description: request.description,
        created_by: user.id,
    };

    match secret_manager.create_secret(&db, create_params).await {
        Ok(secret_info) => {
            let response = SecretResponse::from(secret_info);
            Ok((StatusCode::CREATED, axum::Json(response)).into_response())
        }
        Err(OxyError::SecretManager(msg)) if msg.contains("already exists") => {
            Ok((StatusCode::CONFLICT, axum::Json(json!({ "error": msg }))).into_response())
        }
        Err(OxyError::SecretManager(msg)) => {
            Ok((StatusCode::BAD_REQUEST, axum::Json(json!({ "error": msg }))).into_response())
        }
        Err(e) => {
            tracing::error!("Failed to create secret: {}", e);
            Ok((
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(json!({ "error": "Failed to create secret" })),
            )
                .into_response())
        }
    }
}

pub async fn bulk_create_secrets(
    _: WorkspaceAdmin,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(workspace_id): Path<Uuid>,
    extract::Json(request): extract::Json<BulkCreateSecretsRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    if let Err(validation_errors) = request.validate() {
        tracing::warn!(
            "Bulk secret creation validation failed: {}",
            validation_errors
        );
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            axum::Json(json!({
                "error": "Validation failed",
                "details": validation_errors.to_string()
            })),
        )
            .into_response());
    }

    let db = establish_connection().await.map_err(|e| {
        tracing::error!("Failed to establish database connection: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let secret_manager = SecretManagerService::new(workspace_id);
    let mut created_secrets = Vec::new();
    let mut failed_secrets = Vec::new();

    // Process each secret individually
    for secret_request in request.secrets {
        let create_params = CreateSecretParams {
            name: secret_request.name.clone(),
            value: secret_request.value.clone(),
            description: secret_request.description.clone(),
            created_by: user.id,
        };

        match secret_manager.create_secret(&db, create_params).await {
            Ok(secret_info) => {
                let response = SecretResponse::from(secret_info);
                created_secrets.push(response);
            }
            Err(e) => {
                let error_msg = match e {
                    OxyError::SecretManager(msg) => msg,
                    _ => "Failed to create secret".to_string(),
                };
                failed_secrets.push(FailedSecret {
                    secret: secret_request,
                    error: error_msg,
                });
            }
        }
    }

    let response = BulkCreateSecretsResponse {
        created_secrets,
        failed_secrets,
    };

    // Return 201 if all succeeded, 207 (Multi-Status) if partial success, 400 if all failed
    let status_code = if response.failed_secrets.is_empty() {
        StatusCode::CREATED
    } else if !response.created_secrets.is_empty() {
        StatusCode::MULTI_STATUS
    } else {
        StatusCode::BAD_REQUEST
    };

    Ok((status_code, axum::Json(response)).into_response())
}

/// A non-production app secret (`apps/<id>/<env>/<KEY>`): staff-only, so these
/// tenant routes neither list it nor reach it by id — it answers 404 as a row
/// that does not exist would.
fn staff_only(name: &str) -> bool {
    oxy_custom_apps::secret_scope::is_non_production_app_secret(name)
}

/// List all secrets (without values)
pub async fn list_secrets(
    _: WorkspaceAdmin,
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
    Path(workspace_id): Path<Uuid>,
) -> Result<impl IntoResponse, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("Failed to establish database connection: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let secret_manager = SecretManagerService::new(workspace_id);
    match secret_manager.list_secrets(&db).await {
        Ok(secrets) => {
            let secrets: Vec<_> = secrets
                .into_iter()
                .filter(|s| !staff_only(&s.name))
                .collect();
            // Resolve created_by and updated_by UUIDs → emails in one batch query
            let user_ids: Vec<Uuid> = secrets
                .iter()
                .flat_map(|s| std::iter::once(s.created_by).chain(s.updated_by))
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            let email_map = resolve_user_emails(&db, &user_ids).await;

            let secret_responses: Vec<SecretResponse> = secrets
                .into_iter()
                .map(|s| {
                    let created_by_email = email_map.get(&s.created_by).cloned();
                    let updated_by_email = s.updated_by.and_then(|id| email_map.get(&id).cloned());
                    SecretResponse {
                        created_by_email,
                        updated_by_email,
                        ..SecretResponse::from(s)
                    }
                })
                .collect();

            let response = SecretListResponse {
                total: secret_responses.len(),
                secrets: secret_responses,
            };

            Ok((StatusCode::OK, axum::Json(response)).into_response())
        }
        Err(e) => {
            tracing::error!("Failed to list secrets: {}", e);
            Ok((
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(json!({ "error": "Failed to list secrets" })),
            )
                .into_response())
        }
    }
}

/// Get secret metadata by ID (without value)
pub async fn get_secret(
    _: WorkspaceAdmin,
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
    Path((workspace_id, id)): Path<(Uuid, String)>,
) -> Result<impl IntoResponse, StatusCode> {
    let secret_id = match Uuid::parse_str(&id) {
        Ok(uuid) => uuid,
        Err(_) => {
            return Ok((
                StatusCode::BAD_REQUEST,
                axum::Json(json!({ "error": "Invalid secret ID format" })),
            )
                .into_response());
        }
    };

    let secret_manager = SecretManagerService::new(workspace_id);

    match secret_manager.get_secret_by_id(secret_id).await {
        Some(secret) if !staff_only(&secret.name) => {
            Ok((StatusCode::OK, axum::Json(SecretResponse::from(secret))).into_response())
        }
        _ => Ok((
            StatusCode::NOT_FOUND,
            axum::Json(json!({ "error": "Secret not found" })),
        )
            .into_response()),
    }
}

pub async fn update_secret(
    _: WorkspaceAdmin,
    actor: RequestActor,
    Path((workspace_id, id)): Path<(Uuid, String)>,
    extract::Json(request): extract::Json<UpdateSecretRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    if request.value.is_none() && request.description.is_none() {
        return Ok((
            StatusCode::BAD_REQUEST,
            axum::Json(
                json!({ "error": "At least one of 'value' or 'description' must be provided" }),
            ),
        )
            .into_response());
    }

    if let Err(validation_errors) = request.validate() {
        tracing::warn!("Secret update validation failed: {}", validation_errors);
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            axum::Json(json!({
                "error": "Validation failed",
                "details": validation_errors.to_string()
            })),
        )
            .into_response());
    }

    let secret_id = match Uuid::parse_str(&id) {
        Ok(uuid) => uuid,
        Err(_) => {
            return Ok((
                StatusCode::BAD_REQUEST,
                axum::Json(json!({ "error": "Invalid secret ID format" })),
            )
                .into_response());
        }
    };

    let db = establish_connection().await.map_err(|e| {
        tracing::error!("Failed to establish database connection: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let secret_manager = SecretManagerService::new(workspace_id);

    let Some(secret) = secret_manager
        .get_secret_by_id(secret_id)
        .await
        .filter(|s| !staff_only(&s.name))
    else {
        return Ok((
            StatusCode::NOT_FOUND,
            axum::Json(json!({ "error": "Secret not found" })),
        )
            .into_response());
    };

    let rotated = request.value.is_some();
    let update_params = UpdateSecretParams {
        value: request.value,
        description: request.description,
        updated_by: actor.id,
    };

    match secret_manager
        .update_secret(&db, &secret.name, update_params)
        .await
    {
        Ok(updated_secret) => {
            if rotated {
                audit_if_app_secret(&db, &secret.name, ProjectSecretWrite::Rotated, &actor).await;
            }
            Ok((
                StatusCode::OK,
                axum::Json(SecretResponse::from(updated_secret)),
            )
                .into_response())
        }
        Err(OxyError::SecretManager(msg)) => {
            Ok((StatusCode::BAD_REQUEST, axum::Json(json!({ "error": msg }))).into_response())
        }
        Err(e) => {
            tracing::error!("Failed to update secret: {}", e);
            Ok((
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(json!({ "error": "Failed to update secret" })),
            )
                .into_response())
        }
    }
}

/// Environment variable name prefixes that belong to the authentication layer
/// (Okta, Google OAuth, magic-link) or Oxy's own infrastructure config.
/// These are server-side secrets that have no business appearing in the
/// per-workspace secrets panel — users should never need to view or override them there.
const AUTH_ENV_VAR_PREFIXES: &[&str] = &[
    "GOOGLE_CLIENT_",  // GOOGLE_CLIENT_ID, GOOGLE_CLIENT_SECRET
    "OKTA_",           // OKTA_CLIENT_ID, OKTA_CLIENT_SECRET, OKTA_DOMAIN
    "MAGIC_LINK_",     // all magic-link config vars
    "OXY_",            // internal Oxy infrastructure vars (OXY_DATABASE_URL, OXY_OWNER, …)
    "GIT_REPOSITORY_", // GIT_REPOSITORY_URL
    "GITHUB_",         // all GitHub vars: app config, OAuth login, webhooks, CI env vars
];

fn is_auth_env_var(name: &str) -> bool {
    AUTH_ENV_VAR_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

pub async fn delete_secret(
    _: WorkspaceAdmin,
    actor: RequestActor,
    Path((workspace_id, id)): Path<(Uuid, String)>,
) -> Result<impl IntoResponse, StatusCode> {
    let secret_id = match Uuid::parse_str(&id) {
        Ok(uuid) => uuid,
        Err(_) => {
            return Ok((
                StatusCode::BAD_REQUEST,
                axum::Json(json!({ "error": "Invalid secret ID format" })),
            )
                .into_response());
        }
    };

    let db = establish_connection().await.map_err(|e| {
        tracing::error!("Failed to establish database connection: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let secret_manager = SecretManagerService::new(workspace_id);

    let Some(secret) = secret_manager
        .get_secret_by_id(secret_id)
        .await
        .filter(|s| !staff_only(&s.name))
    else {
        return Ok((
            StatusCode::NOT_FOUND,
            axum::Json(json!({ "error": "Secret not found" })),
        )
            .into_response());
    };

    match secret_manager.delete_secret(&db, &secret.name).await {
        Ok(()) => {
            audit_if_app_secret(&db, &secret.name, ProjectSecretWrite::Deleted, &actor).await;
            Ok((StatusCode::NO_CONTENT, axum::Json(json!({}))).into_response())
        }
        Err(OxyError::SecretManager(msg)) => {
            Ok((StatusCode::BAD_REQUEST, axum::Json(json!({ "error": msg }))).into_response())
        }
        Err(e) => {
            tracing::error!("Failed to delete secret: {}", e);
            Ok((
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(json!({ "error": "Failed to delete secret" })),
            )
                .into_response())
        }
    }
}

/// Reveal the plaintext value of a DB secret by ID (WorkspaceAdmin only).
pub async fn reveal_secret(
    _: WorkspaceAdmin,
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
    Path((workspace_id, id)): Path<(Uuid, String)>,
) -> Result<impl IntoResponse, StatusCode> {
    let secret_id = match Uuid::parse_str(&id) {
        Ok(uuid) => uuid,
        Err(_) => {
            return Ok((
                StatusCode::BAD_REQUEST,
                axum::Json(json!({ "error": "Invalid secret ID format" })),
            )
                .into_response());
        }
    };

    let secret_manager = SecretManagerService::new(workspace_id);
    if let Some(secret) = secret_manager.get_secret_by_id(secret_id).await
        && staff_only(&secret.name)
    {
        return Ok((
            StatusCode::NOT_FOUND,
            axum::Json(json!({ "error": "Secret not found" })),
        )
            .into_response());
    }

    // 404 and 500 are different answers and the caller acts on them differently:
    // a missing secret gets re-created, an unreadable one gets investigated. The
    // old code returned 404 for both, under a message that admitted the
    // ambiguity ("Secret not found or decryption failed") while the status code
    // asserted one half of it.
    match secret_manager.get_secret_value_by_id(secret_id).await {
        Ok(Some(value)) => {
            Ok((StatusCode::OK, axum::Json(json!({ "value": value }))).into_response())
        }
        Ok(None) => Ok((
            StatusCode::NOT_FOUND,
            axum::Json(json!({ "error": "Secret not found" })),
        )
            .into_response()),
        Err(e) => {
            tracing::error!(secret_id = %secret_id, "reveal_secret failed: {e}");
            Ok((
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(json!({
                    "error": "Secret exists but could not be read by this instance",
                    "detail": e.to_string(),
                })),
            )
                .into_response())
        }
    }
}
