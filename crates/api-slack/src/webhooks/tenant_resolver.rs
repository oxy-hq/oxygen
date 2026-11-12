use crate::services::installations::InstallationsService;
use entity::slack_installations::Model as InstallationRow;
use oxy_shared::errors::OxyError;

pub struct ResolvedTenant {
    pub installation: InstallationRow,
    pub bot_token: String,
}

/// Resolve an installed Slack tenant. Returns None if the team isn't installed
/// or its install is revoked — caller should 200-drop.
pub async fn resolve(team_id: &str) -> Result<Option<ResolvedTenant>, OxyError> {
    let Some(inst) = InstallationsService::find_active_by_team(team_id).await? else {
        return Ok(None);
    };
    let bot_token = InstallationsService::decrypt_bot_token(&inst).await?;
    Ok(Some(ResolvedTenant {
        installation: inst,
        bot_token,
    }))
}
