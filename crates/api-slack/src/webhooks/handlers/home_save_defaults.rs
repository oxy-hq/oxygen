use crate::services::installations::InstallationsService;
use crate::services::user_links::UserLinksService;
use crate::services::user_preferences::{UpsertPrefs, UserPreferencesService};
use crate::types::InteractivityPayload;
use oxy_shared::errors::OxyError;
use uuid::Uuid;

pub async fn handle(payload: &InteractivityPayload) -> Result<(), OxyError> {
    let Some(view) = &payload.view else {
        return Ok(());
    };
    let state = &view["state"]["values"];

    let workspace_id = state
        .as_object()
        .and_then(|o| {
            o.values().find_map(|v| {
                v.get("slack_home_pick_workspace")
                    .and_then(|s| s.get("selected_option"))
                    .and_then(|o| o.get("value"))
                    .and_then(|v| v.as_str())
            })
        })
        .and_then(|s| Uuid::parse_str(s).ok());

    // Parse agent selection: value format is "<workspace_uuid>|<agent_path>".
    // If present, it takes precedence over the workspace-only selection.
    let agent_raw = state.as_object().and_then(|o| {
        o.values().find_map(|v| {
            v.get("slack_home_pick_agent")
                .and_then(|s| s.get("selected_option"))
                .and_then(|o| o.get("value"))
                .and_then(|v| v.as_str())
        })
    });

    let (final_workspace_id, agent_path) = if let Some(val) = agent_raw {
        let mut parts = val.splitn(2, '|');
        let ws = parts.next().and_then(|s| Uuid::parse_str(s).ok());
        let agent = parts.next().map(|s| s.to_string());
        // Agent selection determines workspace; fall back to workspace picker if parse fails.
        (ws.or(workspace_id), agent)
    } else {
        (workspace_id, None)
    };

    let Some(inst) = InstallationsService::find_active_by_team(&payload.team.id).await? else {
        return Ok(());
    };
    let Some(link) = UserLinksService::find(inst.id, &payload.user.id).await? else {
        return Ok(());
    };
    UserPreferencesService::upsert(UpsertPrefs {
        user_link_id: link.id,
        default_workspace_id: final_workspace_id,
        default_agent_path: agent_path,
    })
    .await?;
    Ok(())
}
