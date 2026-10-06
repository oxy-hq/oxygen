// crates/app/src/server/api/github/account.rs
use axum::{extract::Query, http::StatusCode, response::Json as ResponseJson};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use entity::github_accounts;
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;

use super::state::{Flow, StatePayload, bounce_redirect, encode_state};

// ──────────────────────── DTOs ────────────────────────

#[derive(Debug, Serialize)]
pub struct AccountStatus {
    pub connected: bool,
    pub github_login: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct OauthUrlQuery {
    pub org_id: Uuid,
    pub origin: String,
}

#[derive(Debug, Serialize)]
pub struct OauthUrlResponse {
    pub url: String,
}

// ──────────────────────── Handlers ────────────────────────

/// GET /user/github/account — returns whether the user has a connected GitHub account.
pub async fn get_account(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
) -> Result<ResponseJson<AccountStatus>, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("failed to establish database connection: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let row = github_accounts::Entity::find()
        .filter(github_accounts::Column::UserId.eq(user.id))
        .one(&db)
        .await
        .map_err(|e| {
            tracing::error!("failed to query github_accounts for user {}: {e}", user.id);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(ResponseJson(match row {
        Some(a) => AccountStatus {
            connected: true,
            github_login: Some(a.github_login),
        },
        None => AccountStatus {
            connected: false,
            github_login: None,
        },
    }))
}

/// DELETE /user/github/account — disconnect the user's GitHub account (does not
/// delete any namespaces the user created — those are org-scoped).
pub async fn delete_account(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
) -> Result<StatusCode, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("failed to establish database connection: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    github_accounts::Entity::delete_many()
        .filter(github_accounts::Column::UserId.eq(user.id))
        .exec(&db)
        .await
        .map_err(|e| {
            tracing::error!("failed to delete github_accounts for user {}: {e}", user.id);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /user/github/account/oauth-url?org_id={uuid}
/// Mints an HMAC-signed state with `flow=oauth` and returns the GitHub authorize URL.
pub async fn get_oauth_url(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Query(q): Query<OauthUrlQuery>,
) -> Result<ResponseJson<OauthUrlResponse>, StatusCode> {
    // No path names the org, so the request's API token is asked here: an org
    // that blocked it (API-tokens design §5) is not one it starts a flow for.
    user.require_org_reach(q.org_id)?;
    let state = encode_state(&StatePayload {
        org_id: q.org_id,
        flow: Flow::Oauth,
    })
    .map_err(StatusCode::from)?;

    let client_id = std::env::var("GITHUB_CLIENT_ID").map_err(|e| {
        tracing::error!("GITHUB_CLIENT_ID env var not set: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Bounce proxy (local multi-instance dev): redirect_uri points at the proxy
    // and the instance origin rides along in `state`; otherwise straight to the
    // instance origin with no suffix.
    let (redirect_base, state_suffix) = bounce_redirect(&q.origin);
    let state = format!("{state}{state_suffix}");
    let redirect_uri = format!("{redirect_base}/github/callback");

    let url = format!(
        "https://github.com/login/oauth/authorize?client_id={}&redirect_uri={}&state={}&scope=read:user,read:org",
        urlencoding::encode(&client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode(&state),
    );

    Ok(ResponseJson(OauthUrlResponse { url }))
}
