use crate::resolution::user::{ResolvedUser, resolve as resolve_user};
use crate::services::user_preferences::{UpsertPrefs, UserPreferencesService};
use crate::types::{InteractivityAction, InteractivityPayload};
use crate::webhooks::tenant_resolver;
use oxy_shared::errors::OxyError;

/// Handle `slack_make_default` interactivity action.
///
/// Parses `action.value` as `<workspace_uuid>|<agent_path>` and persists
/// the workspace + agent as the user's defaults via UserPreferencesService.
pub async fn handle(
    payload: &InteractivityPayload,
    action: &InteractivityAction,
) -> Result<(), OxyError> {
    let value = action.value.as_deref().unwrap_or_default();

    let Some((workspace_uuid_str, agent_path)) = value.split_once('|') else {
        tracing::warn!("make_default: malformed action value: {value}");
        return Ok(());
    };

    let workspace_id = match workspace_uuid_str.parse::<uuid::Uuid>() {
        Ok(id) => id,
        Err(_) => {
            tracing::warn!("make_default: invalid workspace uuid: {workspace_uuid_str}");
            return Ok(());
        }
    };

    let team_id = &payload.team.id;
    let Some(tenant) = tenant_resolver::resolve(team_id).await? else {
        tracing::warn!("make_default: unknown team {team_id}");
        return Ok(());
    };

    let slack_user_id = &payload.user.id;
    let user_link = match resolve_user(&tenant.installation, slack_user_id).await? {
        ResolvedUser::Linked(link) => link,
        ResolvedUser::Unlinked => {
            tracing::warn!(
                slack_user_id,
                "make_default: user not linked — cannot persist preferences"
            );
            return Ok(());
        }
    };

    UserPreferencesService::upsert(UpsertPrefs {
        user_link_id: user_link.id,
        default_workspace_id: Some(workspace_id),
        default_agent_path: Some(agent_path.to_string()),
    })
    .await?;

    tracing::info!(
        user_link_id = %user_link.id,
        workspace_id = %workspace_id,
        agent_path,
        "make_default: preferences saved"
    );

    Ok(())
}
