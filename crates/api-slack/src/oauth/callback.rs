use crate::client::SlackClient;
use crate::config::SlackConfig;
use crate::oauth::state::{OauthStateService, StateKind};
use crate::services::installations::{InstallationsService, UpsertInstallation};
use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use entity::org_members;
use entity::org_members::OrgRole;
use oxy::database::client::establish_connection;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

/// GET /api/slack/oauth/callback
pub async fn callback(Query(q): Query<CallbackQuery>) -> Response {
    if let Some(err) = q.error {
        return (
            StatusCode::BAD_REQUEST,
            format!("Slack install cancelled: {err}"),
        )
            .into_response();
    }
    let code = match q.code {
        Some(c) => c,
        None => return (StatusCode::BAD_REQUEST, "missing code").into_response(),
    };
    let nonce = match q.state {
        Some(s) => s,
        None => return (StatusCode::BAD_REQUEST, "missing state").into_response(),
    };

    let cfg = match SlackConfig::from_env().into_runtime() {
        Some(c) => c,
        None => return (StatusCode::SERVICE_UNAVAILABLE, "Slack disabled").into_response(),
    };

    let state = match OauthStateService::consume(&nonce, StateKind::Install).await {
        Ok(s) => s,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };

    let org_id = state.org_id.expect("install state has org_id");
    let oxy_user_id = state.oxy_user_id.expect("install state has oxy_user_id");

    // Re-verify admin at callback time.
    let conn = match establish_connection().await {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let mem = org_members::Entity::find()
        .filter(org_members::Column::OrgId.eq(org_id))
        .filter(org_members::Column::UserId.eq(oxy_user_id))
        .one(&conn)
        .await;
    let membership = match mem {
        Ok(Some(m)) => m,
        Ok(None) => return (StatusCode::FORBIDDEN, "no longer a member").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    if !matches!(membership.role, OrgRole::Owner | OrgRole::Admin) {
        return (StatusCode::FORBIDDEN, "admin role required").into_response();
    }

    // Exchange code.
    let client = SlackClient::new();
    let redirect_uri = format!("{}/api/slack/oauth/callback", cfg.app_base_url);
    let access = match client
        .oauth_v2_access(&cfg.client_id, &cfg.client_secret, &code, &redirect_uri)
        .await
    {
        Ok(a) => a,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("oauth exchange: {e}")).into_response(),
    };

    if let Err(e) = InstallationsService::upsert(UpsertInstallation {
        org_id,
        team_id: access.team.id.clone(),
        team_name: access.team.name,
        enterprise_id: access.enterprise.as_ref().map(|e| e.id.clone()),
        bot_user_id: access.bot_user_id,
        bot_token: access.access_token,
        scopes: access.scope,
        installed_by_user_id: oxy_user_id,
        installed_by_slack_user_id: access.authed_user.id,
    })
    .await
    {
        // Conflict → return 409 with friendly message.
        return (StatusCode::CONFLICT, e.to_string()).into_response();
    }

    // Fetch org slug for the redirect.
    let org = entity::organizations::Entity::find_by_id(org_id)
        .one(&conn)
        .await
        .ok()
        .flatten();
    let slug = org.map(|o| o.slug).unwrap_or_default();
    // Redirect to the org root (the only frontend-routed path for an org) with
    // a query param the AppSidebar footer watches for — it auto-opens the
    // settings dialog on the Integration tab and shows a success toast.
    //
    // There is no /orgs/<slug>/settings/... route on the frontend — settings
    // live in a modal dialog, not a URL-addressable page.
    Redirect::to(&format!("{}/{slug}?slack_installed=ok", cfg.app_base_url)).into_response()
}
