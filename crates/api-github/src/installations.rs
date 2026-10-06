// crates/app/src/server/api/github/installations.rs
use axum::{extract::Query, http::StatusCode, response::Json as ResponseJson};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use entity::github_accounts;
use oxy::database::client::establish_connection;
use oxy::github::client::GitHubClient;
use oxy_auth::extractor::AuthenticatedUserExtractor;

use super::state::{Flow, StatePayload, bounce_redirect, encode_state};

// ──────────────────────── DTOs ────────────────────────

#[derive(Debug, Deserialize)]
pub struct NewInstallationUrlQuery {
    pub org_id: Uuid,
    pub origin: String,
}

#[derive(Debug, Serialize)]
pub struct NewInstallationUrlResponse {
    pub url: String,
}

#[derive(Debug, Serialize)]
pub struct UserInstallation {
    pub id: i64,
    pub account_login: String,
    pub account_type: String,
}

// ──────────────────────── Handlers ────────────────────────
/// GET /user/github/installations/new-url?org_id={uuid}
/// Returns a URL pointing at `https://github.com/apps/{slug}/installations/new`
/// with an HMAC-signed `state` carrying `flow=install` and the target org.
pub async fn get_new_installation_url(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Query(q): Query<NewInstallationUrlQuery>,
) -> Result<ResponseJson<NewInstallationUrlResponse>, StatusCode> {
    // No path names the org, so the request's API token is asked here: an org
    // that blocked it (API-tokens design §5) is not one it installs into.
    user.require_org_reach(q.org_id)?;
    let state = encode_state(&StatePayload {
        org_id: q.org_id,
        flow: Flow::Install,
    })
    .map_err(StatusCode::from)?;

    let slug = std::env::var("GITHUB_APP_SLUG").map_err(|e| {
        tracing::error!("GITHUB_APP_SLUG env var not set: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Bounce proxy (local multi-instance dev): redirect_uri points at the proxy
    // and the instance origin rides along in `state`; otherwise straight to the
    // instance origin with no suffix.
    let (redirect_base, state_suffix) = bounce_redirect(&q.origin);
    let state = format!("{state}{state_suffix}");
    let redirect_uri = format!("{redirect_base}/github/callback");

    let url = format!(
        "https://github.com/apps/{}/installations/new?state={}&redirect_uri={}",
        urlencoding::encode(&slug),
        urlencoding::encode(&state),
        urlencoding::encode(&redirect_uri),
    );
    Ok(ResponseJson(NewInstallationUrlResponse { url }))
}

/// GET /user/github/installations — list installations the user has access to
/// on GitHub. Uses the user's stored OAuth token.
pub async fn list_installations(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
) -> Result<ResponseJson<Vec<UserInstallation>>, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("failed to establish database connection: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let account = github_accounts::Entity::find()
        .filter(github_accounts::Column::UserId.eq(user.id))
        .one(&db)
        .await
        .map_err(|e| {
            tracing::error!("failed to query github_accounts for user {}: {e}", user.id);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or_else(|| {
            tracing::warn!("no github account connected for user {}", user.id);
            StatusCode::UNAUTHORIZED
        })?;

    let client = GitHubClient::from_token(account.oauth_token).map_err(|e| {
        tracing::error!(
            "failed to initialize GitHub client for user {}: {e}",
            user.id
        );
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let installations = client.list_user_installations().await.map_err(|e| {
        tracing::error!(
            "failed to list github user installations for user {}: {e}",
            user.id
        );
        StatusCode::BAD_GATEWAY
    })?;

    Ok(ResponseJson(
        installations
            .into_iter()
            .map(|i| UserInstallation {
                id: i.id,
                account_login: i.slug,
                account_type: i.owner_type,
            })
            .collect(),
    ))
}
