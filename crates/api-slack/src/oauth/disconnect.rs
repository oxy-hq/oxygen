use crate::client::SlackClient;
use crate::services::installations::InstallationsService;
use axum::http::StatusCode;
use oxy_app::surface::role_guards::OrgAdmin;

/// Disconnecting an org's Slack install is an org-admin action, so it takes the
/// [`OrgAdmin`] extractor rather than restating the ring — the guard's own check is
/// byte-identical to the `matches!(role, Owner | Admin)` this used to hand-roll, and
/// going through it means this handler follows the ring if the ring ever moves.
pub async fn disconnect(OrgAdmin(ctx): OrgAdmin) -> Result<StatusCode, (StatusCode, String)> {
    let inst = InstallationsService::find_active_by_org(ctx.org.id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let inst = match inst {
        Some(i) => i,
        None => return Ok(StatusCode::NO_CONTENT),
    };
    let bot_token = InstallationsService::decrypt_bot_token(&inst)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    // Best-effort revoke — if Slack returns an error we still mark locally revoked.
    let _ = SlackClient::new().auth_revoke(&bot_token).await;
    InstallationsService::revoke(inst.id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}
