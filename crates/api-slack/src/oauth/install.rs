use crate::config::SlackConfig;
use crate::oauth::state::{CreateInstallState, OauthStateService};
use crate::scopes;
use axum::Json;
use axum::http::StatusCode;
use oxy_app::surface::role_guards::OrgAdmin;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct InstallUrlResponse {
    /// Pre-built `slack.com/oauth/v2/authorize?…` URL. The frontend
    /// should `window.location.href = url` to hand off to Slack.
    pub url: String,
}

/// POST /api/orgs/:org_id/slack/install
///
/// Returns the Slack OAuth authorize URL as JSON. The frontend calls this
/// via XHR (so the Authorization header is attached by the axios interceptor)
/// and then navigates the browser to `response.url`. Requires Owner or Admin
/// role in the org.
///
/// This was previously a 302 redirect, but browser-level navigations
/// (`window.location.href = '/api/…'`) do NOT carry the JWT from
/// localStorage — only XHRs do. Splitting the handshake lets the
/// authenticated call build the URL and the browser hand off to Slack
/// without needing auth on the navigation itself.
pub async fn start_install(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    OrgAdmin(ctx): OrgAdmin,
) -> Result<Json<InstallUrlResponse>, (StatusCode, String)> {
    let cfg = SlackConfig::from_env().into_runtime().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Slack integration not configured on this server".to_string(),
    ))?;

    let nonce = OauthStateService::create_install(CreateInstallState {
        org_id: ctx.org.id,
        oxy_user_id: user.id,
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let redirect_uri = format!("{}/api/slack/oauth/callback", cfg.app_base_url);
    let url = format!(
        "https://slack.com/oauth/v2/authorize?client_id={}&scope={}&redirect_uri={}&state={}",
        urlencoding::encode(&cfg.client_id),
        urlencoding::encode(&scopes::scopes_csv()),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode(&nonce),
    );
    Ok(Json(InstallUrlResponse { url }))
}
