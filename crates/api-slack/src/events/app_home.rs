use crate::client::SlackClient;
use crate::config::SlackConfig;
use crate::home::view::{LinkedHomeInput, linked_view, unlinked_view};
use crate::linking::magic_link::new_link_url;
use crate::resolution::user::{ResolvedUser, resolve};
use crate::resolution::workspace_agent::build_workspace_summaries;
use crate::services::user_preferences::UserPreferencesService;
use entity::prelude::{Organizations, Users};
use entity::slack_installations::Model as InstallationRow;
use oxy::database::client::establish_connection;
use oxy_shared::errors::OxyError;
use sea_orm::EntityTrait;

pub async fn handle(
    installation: InstallationRow,
    bot_token: String,
    slack_user_id: String,
) -> Result<(), OxyError> {
    let client = SlackClient::new();
    let base_url = SlackConfig::cached()
        .as_runtime()
        .map(|c| c.app_base_url.clone())
        .unwrap_or_default();

    let view = match resolve(&installation, &slack_user_id).await? {
        ResolvedUser::Unlinked => {
            // App Home context has no channel/thread — no post-connection confirmation
            // ephemeral to target; pass None for both.
            let url = new_link_url(&installation.slack_team_id, &slack_user_id, None, None).await?;
            unlinked_view(&url)
        }
        ResolvedUser::Linked(link) => {
            let conn = establish_connection().await?;
            let user = Users::find_by_id(link.oxy_user_id)
                .one(&conn)
                .await
                .map_err(|e| OxyError::DBError(e.to_string()))?
                .ok_or_else(|| OxyError::DBError("user gone".into()))?;
            let org = Organizations::find_by_id(installation.org_id)
                .one(&conn)
                .await
                .map_err(|e| OxyError::DBError(e.to_string()))?;
            let prefs = UserPreferencesService::get(link.id).await?;
            let workspaces = build_workspace_summaries(&installation)
                .await
                .unwrap_or_default();
            linked_view(LinkedHomeInput {
                email: user.email.as_deref().unwrap_or(""),
                org_name: org.as_ref().map(|o| o.name.as_str()).unwrap_or(""),
                workspaces: &workspaces,
                default_workspace_id: prefs.as_ref().and_then(|p| p.default_workspace_id),
                default_agent_path: prefs.as_ref().and_then(|p| p.default_agent_path.as_deref()),
                app_base_url: &base_url,
            })
        }
    };

    client
        .views_publish(&bot_token, &slack_user_id, view)
        .await?;
    Ok(())
}
