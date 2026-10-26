// crates/app/src/server/api/github/callback.rs
use axum::{Json, http::StatusCode, response::Json as ResponseJson};
use chrono::Utc;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use entity::github_accounts;
use oxy::database::client::establish_connection;
use oxy::github::client::{GitHubClient, exchange_oauth_code};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AuthenticatedUser;

use super::attach::{
    AttachError, attach_namespace, callers_installation, connect, require_org_admin,
};
use super::state::{Flow, decode_state_for};

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
) -> Result<ResponseJson<CallbackResponse>, AttachError> {
    // GitHub sends the browser back to whichever session it lands in, so the
    // state names who started the flow and is completed by that person only.
    //
    // A stale, tampered or old-format state is a `400`, never a `401`: the web
    // app reads any `401` as a dead session and signs the person out, when all
    // they need is to start the flow again. Someone else's state stays `403`
    // (see `StateError`'s `StatusCode` mapping).
    let payload = decode_state_for(&body.state, user.id).map_err(|e| {
        tracing::warn!(user = %user.id, "refused GitHub callback state: {e:?}");
        StatusCode::from(e)
    })?;
    // The org rides in the signed state, not the path. A state minted for an
    // org that has blocked the request's API token (API-tokens design §5) is
    // not completed with it — the install flow writes a namespace there.
    user.require_org_reach(payload.org_id)?;

    match payload.flow {
        Flow::Oauth => Ok(handle_oauth(user.id, body.code).await?),
        Flow::Install => handle_install(&user, payload.org_id, body.installation_id).await,
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

/// The install redirect's `installation_id` is a query parameter anyone can
/// edit, so it is a claim: it is attached only for an org admin, asked again
/// now (they may have been demoted since the state was minted), and only if
/// the caller's own GitHub account lists it. See [`crate::attach`].
async fn handle_install(
    user: &AuthenticatedUser,
    org_id: Uuid,
    installation_id: Option<i64>,
) -> Result<ResponseJson<CallbackResponse>, AttachError> {
    let user_id = user.id;
    let installation_id = installation_id.ok_or_else(|| {
        tracing::warn!(
            "GitHub install callback missing 'installation_id' for user {user_id} org {org_id}"
        );
        StatusCode::BAD_REQUEST
    })?;

    require_org_admin(user, org_id).await?;
    let db = connect().await?;
    let installation = callers_installation(&db, user_id, installation_id).await?;
    let ns = attach_namespace(&db, org_id, user_id, installation).await?;

    Ok(ResponseJson(CallbackResponse::Install {
        namespace_id: ns.id,
    }))
}
