//! `POST /api/{workspace_id}/api-keys/{id}/extend` — push out a key's expiry
//! without changing the key (API-tokens design §3.6).
//!
//! Browser session only: a key cannot extend itself, so a leaked key cannot
//! keep itself alive. The key must be the caller's (else 404). An expired key
//! can be extended — that revives it where it is already deployed — but a
//! revoked one cannot (409).

use axum::{
    Extension,
    body::Bytes,
    extract::Path,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use oxy::database::client::establish_connection;
use oxy_app_core::audit::RequestActor;
use oxy_auth::extractor::{SessionAction, SessionOnly};
use oxy_auth::{ExtendError, ExtendTo};
use serde_json::json;
use uuid::Uuid;

use super::ApiKeyResponse;
use super::lifecycle::{self, Actor};

/// The contract's 403 body for a token-authenticated caller.
pub struct ExtendKey;
impl SessionAction for ExtendKey {
    const REFUSAL: &'static str = "extend requires a browser session";
}

fn error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, axum::Json(json!({ "error": message.into() }))).into_response()
}

/// Extend an API key's expiry
#[utoipa::path(
    post,
    path = "/{workspace_id}/api-keys/{id}/extend",
    request_body(
        content = serde_json::Value,
        description = "Exactly one of {\"days\": 1-3650}, {\"expires_at\": \"<RFC 3339>\"} or {\"expires_at\": null}"
    ),
    responses(
        (status = 200, description = "The updated API key", body = ApiKeyResponse),
        (status = 400, description = "Invalid body"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Called with an API key or token, not a browser session"),
        (status = 404, description = "Not found, or not the caller's key"),
        (status = 409, description = "The key is revoked"),
        (status = 500, description = "Internal server error")
    ),
    tag = "API Keys",
    params(
        ("workspace_id" = Uuid, Path, description = "Workspace UUID"),
        ("id" = String, Path, description = "API key ID")
    ),
)]
pub async fn extend_api_key(
    _: SessionOnly<ExtendKey>,
    user: RequestActor,
    workspace: Option<Extension<entity::workspaces::Model>>,
    Path((workspace_id, id)): Path<(Uuid, String)>,
    body: Bytes,
) -> Response {
    let Ok(key_id) = Uuid::parse_str(&id) else {
        return error(StatusCode::NOT_FOUND, "API key not found");
    };
    let target = match serde_json::from_slice::<serde_json::Value>(&body)
        .map_err(|e| format!("invalid JSON: {e}"))
        .and_then(|v| ExtendTo::from_json(&v))
    {
        Ok(target) => target,
        Err(msg) => return error(StatusCode::BAD_REQUEST, msg),
    };
    let result = async {
        let db = establish_connection().await?;
        let actor = Actor {
            request: &user,
            workspace_id,
            org_id: lifecycle::org_of(&db, workspace, workspace_id).await?,
        };
        lifecycle::extend(&db, &actor, key_id, target).await
    }
    .await;
    match result {
        Ok(extended) => {
            axum::Json(ApiKeyResponse::from(extended.api_key).without_key()).into_response()
        }
        Err(e) => extend_error(e),
    }
}

fn extend_error(e: ExtendError) -> Response {
    match e {
        ExtendError::NotFound => error(StatusCode::NOT_FOUND, "API key not found"),
        ExtendError::Revoked => error(StatusCode::CONFLICT, "API key is revoked"),
        ExtendError::Invalid(msg) => error(StatusCode::BAD_REQUEST, msg),
        ExtendError::Db(e) => {
            tracing::error!("Failed to extend API key: {e}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
        }
    }
}
