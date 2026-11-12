use crate::oauth::state::{OauthStateService, StateKind};
use crate::services::installations::InstallationsService;
use crate::services::user_links::{CreateLink, LinkMethod, UserLinksService};
use axum::Json;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use entity::org_members;
use oxy::database::client::establish_connection;
use oxy_auth::extractor::{AuthenticatedUserExtractor, OptionalAuthenticatedUser};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};

/// Escape a string for safe inclusion in HTML text content or attribute values.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

#[derive(Debug, Deserialize)]
pub struct LandingQuery {
    pub token: String,
}

/// GET /api/slack/link?token=<nonce>
///
/// Shows a confirmation page to the authenticated user. If not logged in,
/// bounces through the login flow first.
pub async fn landing(Query(q): Query<LandingQuery>, user: OptionalAuthenticatedUser) -> Response {
    let OptionalAuthenticatedUser(user) = user;
    let Some(user) = user else {
        // Bounce through login.
        return Redirect::to(&format!(
            "/login?next=/api/slack/link?token={}",
            urlencoding::encode(&q.token)
        ))
        .into_response();
    };

    // Peek at the state (don't consume yet) to render the confirmation page.
    let conn = match establish_connection().await {
        Ok(c) => c,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "db error").into_response(),
    };
    let state = match entity::slack_oauth_states::Entity::find()
        .filter(entity::slack_oauth_states::Column::Nonce.eq(&q.token))
        .one(&conn)
        .await
    {
        Ok(Some(s))
            if s.kind == "user_link"
                && s.consumed_at.is_none()
                && chrono::DateTime::<chrono::Utc>::from(s.expires_at) > chrono::Utc::now() =>
        {
            s
        }
        _ => return (StatusCode::BAD_REQUEST, "invalid or expired link").into_response(),
    };

    let team_id = state.slack_team_id.clone().unwrap_or_default();
    let inst = match InstallationsService::find_active_by_team(&team_id).await {
        Ok(Some(i)) => i,
        _ => return (StatusCode::BAD_REQUEST, "install not active").into_response(),
    };

    let safe_slack_user = html_escape(&state.slack_user_id.clone().unwrap_or_default());
    let safe_team_name = html_escape(&inst.slack_team_name);
    let safe_email = html_escape(user.email.as_deref().unwrap_or(""));
    let safe_token = html_escape(&q.token);
    let html = format!(
        r#"<!doctype html><html><body style="font-family:sans-serif;max-width:480px;margin:48px auto">
           <h2>Connect Slack to Oxygen</h2>
           <p>Connect Slack user <code>{slack_user}</code> in <b>{team_name}</b> to your Oxygen account
              (<code>{email}</code>)?</p>
           <form method="POST" action="/api/slack/link/confirm">
             <input type="hidden" name="token" value="{token}"/>
             <button type="submit" style="padding:8px 16px;background:#3550FF;color:white;border:0;border-radius:4px">Confirm</button>
             <a href="/" style="margin-left:12px">Cancel</a>
           </form></body></html>"#,
        slack_user = safe_slack_user,
        team_name = safe_team_name,
        email = safe_email,
        token = safe_token,
    );
    Html(html).into_response()
}

#[derive(Debug, Deserialize)]
pub struct ConfirmBody {
    pub token: String,
}

#[derive(Serialize)]
pub struct ConfirmResponse {
    pub success: bool,
    pub team_name: String,
}

/// POST /api/slack/link/confirm
///
/// Consumes the state token and creates the user link. Requires the user to
/// be a member of the installation's org.
pub async fn confirm(
    user: AuthenticatedUserExtractor,
    axum::Form(body): axum::Form<ConfirmBody>,
) -> Result<Json<ConfirmResponse>, (StatusCode, String)> {
    let AuthenticatedUserExtractor(user) = user;
    let state = OauthStateService::consume(&body.token, StateKind::UserLink)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    let team_id = state
        .slack_team_id
        .ok_or((StatusCode::BAD_REQUEST, "bad state".into()))?;
    let slack_user_id = state
        .slack_user_id
        .ok_or((StatusCode::BAD_REQUEST, "bad state".into()))?;
    let inst = InstallationsService::find_active_by_team(&team_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::BAD_REQUEST, "install not active".into()))?;

    // Require org membership.
    let conn = establish_connection()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mem = org_members::Entity::find()
        .filter(org_members::Column::OrgId.eq(inst.org_id))
        .filter(org_members::Column::UserId.eq(user.id))
        .one(&conn)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if mem.is_none() {
        // Look up the Oxy org name so the error message is meaningful.
        let org_name = entity::organizations::Entity::find_by_id(inst.org_id)
            .one(&conn)
            .await
            .ok()
            .flatten()
            .map(|o| o.name)
            .unwrap_or_else(|| inst.slack_team_name.clone());
        return Err((
            StatusCode::FORBIDDEN,
            format!("You need to be invited to org '{}' first", org_name),
        ));
    }

    // Capture the originating channel/thread before consuming slack_user_id.
    let origin_channel = state.slack_channel_id.clone();
    let origin_thread = state.slack_thread_ts.clone();

    // Idempotent: if a link already exists for (installation, slack_user), treat as success.
    if UserLinksService::find(inst.id, &slack_user_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .is_none()
    {
        UserLinksService::create(CreateLink {
            installation_id: inst.id,
            slack_user_id: slack_user_id.clone(),
            oxy_user_id: user.id,
            link_method: LinkMethod::MagicLink,
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    // Post a "✅ You're connected!" notification back to the channel where
    // the user originally asked, so they know they can go back and ask
    // their question. Failures are logged-and-swallowed — the link is
    // already established, the notification is best-effort UX polish.
    //
    // DM channels (IDs starting with 'D') do not deliver chat.postEphemeral
    // — Slack silently drops it. In that case we fall back to a regular
    // in-thread message, which is safe because a DM is already private.
    if let (Some(channel), Some(thread_ts)) = (origin_channel, origin_thread) {
        let bot_token_result =
            crate::services::installations::InstallationsService::decrypt_bot_token(&inst).await;
        match bot_token_result {
            Ok(bot_token) => {
                let client = crate::client::SlackClient::new();
                let blocks = serde_json::json!([{
                    "type": "section",
                    "text": {
                        "type": "mrkdwn",
                        "text": "✅ *You're connected!* Go back and ask your question — I'm ready."
                    }
                }]);
                let is_dm = channel.starts_with('D');
                let _ = if is_dm {
                    client
                        .chat_post_message_with_blocks(
                            &bot_token,
                            &channel,
                            "You're connected!",
                            Some(&thread_ts),
                            Some(blocks),
                        )
                        .await
                } else {
                    client
                        .chat_post_ephemeral(
                            &bot_token,
                            &channel,
                            &slack_user_id,
                            blocks,
                            "You're connected!",
                            Some(&thread_ts),
                        )
                        .await
                };
            }
            Err(e) => {
                tracing::warn!("post-confirm notification: failed to decrypt bot token: {e}");
            }
        }
    }

    Ok(Json(ConfirmResponse {
        success: true,
        team_name: inst.slack_team_name,
    }))
}
