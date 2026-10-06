// crates/app/src/server/api/github/callback.rs
use axum::{Json, http::StatusCode, response::Json as ResponseJson};
use chrono::Utc;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use entity::{git_namespaces, github_accounts};
use oxy::database::client::establish_connection;
use oxy::github::app_auth::GitHubAppAuth;
use oxy::github::client::{GitHubClient, exchange_oauth_code};
use oxy_auth::extractor::AuthenticatedUserExtractor;

use super::state::{Flow, decode_state};

#[derive(Debug, Deserialize)]
pub struct CallbackBody {
    pub state: String,
    pub code: Option<String>,
    pub installation_id: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "flow", rename_all = "lowercase")]
pub enum CallbackResponse {
    Oauth { login: String },
    Install { namespace_id: Uuid },
}

/// POST /user/github/callback — authenticated callback completion.
/// The frontend `/github/callback` page calls this after GitHub redirects
/// back. Dispatches on `state.flow` and returns structured JSON.
pub async fn callback(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Json(body): Json<CallbackBody>,
) -> Result<ResponseJson<CallbackResponse>, StatusCode> {
    let payload = decode_state(&body.state).map_err(|e| {
        tracing::warn!("failed to decode GitHub callback state: {e:?}");
        StatusCode::UNAUTHORIZED
    })?;
    // The org rides in the signed state, not the path. A state minted for an
    // org that has blocked the request's API token (API-tokens design §5) is
    // not completed with it — the install flow writes a namespace there.
    user.require_org_reach(payload.org_id)?;

    match payload.flow {
        Flow::Oauth => handle_oauth(user.id, body.code).await,
        Flow::Install => handle_install(user.id, payload.org_id, body.installation_id).await,
    }
}

async fn handle_oauth(
    user_id: Uuid,
    code: Option<String>,
) -> Result<ResponseJson<CallbackResponse>, StatusCode> {
    let code = code.ok_or_else(|| {
        tracing::warn!("GitHub OAuth callback missing 'code' for user {user_id}");
        StatusCode::BAD_REQUEST
    })?;

    let token = exchange_oauth_code(&code).await.map_err(|e| {
        tracing::error!("OAuth code exchange failed for user {user_id}: {e}");
        StatusCode::BAD_GATEWAY
    })?;

    let client = GitHubClient::from_token(token.clone()).map_err(|e| {
        tracing::error!("failed to initialize GitHub client for user {user_id}: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let login = client.get_current_user().await.map_err(|e| {
        tracing::error!("failed to fetch GitHub user info for user {user_id}: {e}");
        StatusCode::BAD_GATEWAY
    })?;

    let db = establish_connection().await.map_err(|e| {
        tracing::error!("failed to establish database connection: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let now = Utc::now().into();
    let existing = github_accounts::Entity::find()
        .filter(github_accounts::Column::UserId.eq(user_id))
        .one(&db)
        .await
        .map_err(|e| {
            tracing::error!("failed to query github_accounts for user {user_id}: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    match existing {
        Some(model) => {
            let mut active: github_accounts::ActiveModel = model.into();
            active.github_login = Set(login.clone());
            active.oauth_token = Set(token);
            active.updated_at = Set(now);
            active.update(&db).await.map_err(|e| {
                tracing::error!("failed to update github_accounts for user {user_id}: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        }
        None => {
            let active = github_accounts::ActiveModel {
                id: Set(Uuid::new_v4()),
                user_id: Set(user_id),
                github_login: Set(login.clone()),
                oauth_token: Set(token),
                created_at: Set(now),
                updated_at: Set(now),
            };
            active.insert(&db).await.map_err(|e| {
                tracing::error!("failed to insert github_accounts for user {user_id}: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        }
    }

    Ok(ResponseJson(CallbackResponse::Oauth { login }))
}

async fn handle_install(
    user_id: Uuid,
    org_id: Uuid,
    installation_id: Option<i64>,
) -> Result<ResponseJson<CallbackResponse>, StatusCode> {
    let installation_id = installation_id.ok_or_else(|| {
        tracing::warn!(
            "GitHub install callback missing 'installation_id' for user {user_id} org {org_id}"
        );
        StatusCode::BAD_REQUEST
    })?;

    let app_auth = GitHubAppAuth::from_env().map_err(|e| {
        tracing::error!("GitHub App environment misconfigured: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let installation = app_auth
        .get_installation_info(&installation_id.to_string())
        .await
        .map_err(|e| {
            tracing::error!(
                "failed to fetch GitHub installation info for installation {installation_id}: {e}"
            );
            StatusCode::BAD_GATEWAY
        })?;

    let db = establish_connection().await.map_err(|e| {
        tracing::error!("failed to establish database connection: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let existing = git_namespaces::Entity::find()
        .filter(git_namespaces::Column::InstallationId.eq(installation_id))
        .filter(git_namespaces::Column::OrgId.eq(org_id))
        .one(&db)
        .await
        .map_err(|e| {
            tracing::error!(
                "failed to query git_namespaces for installation {installation_id} org {org_id}: {e}"
            );
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let namespace_id = match existing {
        Some(model) => model.id,
        None => {
            let id = Uuid::new_v4();
            let active = git_namespaces::ActiveModel {
                id: Set(id),
                installation_id: Set(installation_id),
                name: Set(installation.name),
                owner_type: Set(installation.owner_type),
                provider: Set("github".into()),
                slug: Set(installation.slug),
                oauth_token: Set(String::new()),
                created_by: Set(user_id),
                org_id: Set(Some(org_id)),
            };
            active.insert(&db).await.map_err(|e| {
                tracing::error!(
                    "failed to insert git_namespace for installation {installation_id} org {org_id}: {e}"
                );
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
            id
        }
    };

    Ok(ResponseJson(CallbackResponse::Install { namespace_id }))
}
