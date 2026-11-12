//! `slack_reopen_picker` — the "Wrong workspace?" button in the answer
//! footer. Re-emits the existing ephemeral workspace picker so the user
//! can re-run their question against a different workspace.
//!
//! The original question is forwarded verbatim via `action.value`
//! (base64-encoded). All the heavy lifting — picking a workspace,
//! optionally setting a channel default, spawning the new agent run —
//! is handled by `submit_workspace_picker` once the user confirms.

use crate::client::SlackClient;
use crate::pickers::workspace::workspace_picker_blocks;
use crate::resolution::user::{ResolvedUser, resolve as resolve_user};
use crate::resolution::workspace_agent::build_workspace_summaries;
use crate::types::{InteractivityAction, InteractivityPayload};
use crate::webhooks::handlers::pick_workspace::extract_channel_and_thread;
use crate::webhooks::tenant_resolver;
use oxy_shared::errors::OxyError;

pub async fn handle(
    payload: &InteractivityPayload,
    action: &InteractivityAction,
) -> Result<(), OxyError> {
    let encoded_q = action.value.as_deref().unwrap_or_default();
    if encoded_q.is_empty() {
        tracing::warn!("reopen_picker: missing encoded question in action.value");
        return Ok(());
    }

    let team_id = &payload.team.id;
    let Some(tenant) = tenant_resolver::resolve(team_id).await? else {
        tracing::warn!("reopen_picker: unknown team {team_id}");
        return Ok(());
    };

    let slack_user_id = &payload.user.id;
    match resolve_user(&tenant.installation, slack_user_id).await? {
        ResolvedUser::Linked(_) => {}
        ResolvedUser::Unlinked => {
            tracing::warn!(slack_user_id, "reopen_picker: user not linked");
            return Ok(());
        }
    }

    let Some((channel_id, thread_ts)) = extract_channel_and_thread(payload) else {
        tracing::warn!("reopen_picker: could not extract channel from container");
        return Ok(());
    };

    // Refetch — the workspace list may have grown/shrunk since the original
    // run. If it's now <2, the button shouldn't have been shown but the
    // remote state may have changed; bail quietly.
    let workspaces = build_workspace_summaries(&tenant.installation).await?;
    if workspaces.len() < 2 {
        tracing::info!(
            workspace_count = workspaces.len(),
            "reopen_picker: nothing to switch to, suppressing picker"
        );
        return Ok(());
    }

    let blocks = workspace_picker_blocks(&workspaces, encoded_q);
    let client = SlackClient::new();

    // Slack DM channel IDs start with 'D'. postEphemeral is not delivered in
    // DMs, so we fall back to a regular threaded message (the DM is already
    // private — no information leak).
    if channel_id.starts_with('D') {
        client
            .chat_post_message_with_blocks(
                &tenant.bot_token,
                &channel_id,
                "Pick a different workspace:",
                Some(&thread_ts),
                Some(blocks),
            )
            .await?;
    } else {
        client
            .chat_post_ephemeral(
                &tenant.bot_token,
                &channel_id,
                slack_user_id,
                blocks,
                "Pick a different workspace",
                Some(&thread_ts),
            )
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        InteractivityAction, InteractivityPayload, InteractivityTeam, InteractivityUser,
    };

    fn payload_with_action(action: InteractivityAction) -> InteractivityPayload {
        InteractivityPayload {
            payload_type: "block_actions".into(),
            team: InteractivityTeam { id: "T1".into() },
            user: InteractivityUser { id: "U1".into() },
            channel: None,
            actions: vec![action],
            view: None,
            state: None,
            container: None,
            trigger_id: None,
            response_url: None,
            message: None,
        }
    }

    #[tokio::test]
    async fn handle_returns_ok_when_action_value_missing() {
        // No question encoded → handler logs and returns Ok, no panic, no
        // tenant resolution attempted.
        let action = InteractivityAction {
            action_id: "slack_reopen_picker".into(),
            value: None,
            selected_option: None,
        };
        let payload = payload_with_action(action);
        // Just expect it not to panic; we can't assert tenant lookup result
        // without a database, but the early return must be hit first.
        let result = handle(&payload, &payload.actions[0]).await;
        assert!(result.is_ok(), "expected Ok, got {result:?}");
    }

    #[tokio::test]
    async fn handle_returns_ok_when_action_value_empty_string() {
        let action = InteractivityAction {
            action_id: "slack_reopen_picker".into(),
            value: Some(String::new()),
            selected_option: None,
        };
        let payload = payload_with_action(action);
        let result = handle(&payload, &payload.actions[0]).await;
        assert!(result.is_ok());
    }
}
