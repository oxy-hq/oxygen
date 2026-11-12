use base64::Engine;

use crate::client::SlackClient;
use crate::events::execution::{SlackRunRequest, run_for_slack};
use crate::resolution::user::{ResolvedUser, resolve as resolve_user};
use crate::resolution::workspace_agent::pick_default_agent_path;
use crate::services::channel_defaults::{ChannelDefaultsService, UpsertChannelDefault};
use crate::types::{InteractivityAction, InteractivityPayload};
use crate::webhooks::handlers::pick_workspace::extract_channel_and_thread;
use crate::webhooks::tenant_resolver;
use oxy_app_core::preagg_cache_ctx::PreaggCacheCtx;
use oxy_shared::errors::OxyError;
use uuid::Uuid;

/// Handle `slack_submit_workspace_picker` — the Submit button on the Claude-style picker.
///
/// Payload shape:
/// - `action.value` → base64-encoded original question
/// - `state.values.workspace_block.workspace_select.selected_option.value` → workspace UUID
/// - `state.values.default_block.set_as_default.selected_options` → `[{value:"set_as_default"}]`
///   if the "Set as default for this channel" checkbox was checked
pub async fn handle(
    payload: &InteractivityPayload,
    action: &InteractivityAction,
    // The node's Layer-1 pre-aggregation cache for the run this dispatches. A
    // default carries no cache, so the agent's semantic queries compile to
    // warehouse SQL — the right answer, just without the rollup shortcut.
    preagg: &PreaggCacheCtx,
) -> Result<(), OxyError> {
    // 1. Decode the original question from action.value.
    let encoded_q = action.value.as_deref().unwrap_or_default();
    let question = decode_b64_str(encoded_q).ok_or_else(|| {
        OxyError::ValidationError(
            "submit_workspace_picker: action.value must be base64-encoded question".into(),
        )
    })?;

    // 2. Parse state values.
    let state = payload.state.as_ref().and_then(|s| s.get("values"));
    let workspace_id = parse_workspace_id_from_state(state).ok_or_else(|| {
        OxyError::ValidationError(
            "submit_workspace_picker: missing workspace selection in state".into(),
        )
    })?;
    let set_as_default = parse_set_as_default_from_state(state);

    // 3. Resolve tenant.
    let team_id = &payload.team.id;
    let Some(tenant) = tenant_resolver::resolve(team_id).await? else {
        tracing::warn!("submit_workspace_picker: unknown team {team_id}");
        return Ok(());
    };

    // 4. Resolve user link.
    let slack_user_id = &payload.user.id;
    let user_link = match resolve_user(&tenant.installation, slack_user_id).await? {
        ResolvedUser::Linked(link) => link,
        ResolvedUser::Unlinked => {
            tracing::warn!(slack_user_id, "submit_workspace_picker: user not linked");
            return Ok(());
        }
    };

    // 5. Extract channel + thread from payload.container.
    let Some((channel_id, thread_ts)) = extract_channel_and_thread(payload) else {
        tracing::warn!("submit_workspace_picker: could not extract channel/thread from container");
        return Ok(());
    };

    // 6. If "set as default" was checked, persist the channel default.
    if set_as_default {
        tracing::info!(
            installation_id = %tenant.installation.id,
            channel_id,
            workspace_id = %workspace_id,
            "submit_workspace_picker: persisting channel default"
        );
        ChannelDefaultsService::upsert(UpsertChannelDefault {
            installation_id: tenant.installation.id,
            slack_channel_id: channel_id.clone(),
            workspace_id,
            set_by_user_link_id: user_link.id,
        })
        .await?;
    }

    // 7. Pick the agent for the selected workspace — `defaults.agent` from
    // `config.yml` if configured, otherwise the alphabetically-first one.
    let client = SlackClient::new();
    let bot_token = &tenant.bot_token;
    let Some(agent_path) = pick_default_agent_path(workspace_id).await? else {
        client
            .chat_post_ephemeral(
                bot_token,
                &channel_id,
                slack_user_id,
                serde_json::json!([{
                    "type": "section",
                    "text": {
                        "type": "mrkdwn",
                        "text": format!("No agents found in workspace `{workspace_id}`.")
                    }
                }]),
                "No agents found",
                Some(&thread_ts),
            )
            .await?;
        return Ok(());
    };

    // 8. Delete the picker message before the agent response lands.
    // The response_url is pre-authenticated by Slack so no bot token is needed.
    // We fire this before spawning the agent task so the picker disappears as
    // soon as the user taps Submit, not after the (potentially slow) run.
    if let Some(url) = &payload.response_url {
        client.delete_via_response_url(url).await;
    }

    // 9. Run the agent. Errors surface back to the user via
    // `dispatch_user_event` — without it, a failure inside the spawned
    // task (e.g. workspace build erroring on a misconfigured S3 backend)
    // would just log and the picker would go silent.
    //
    // We clone the dispatch context up front because the request struct
    // moves the originals. Cloning `client` (rather than `SlackClient::new()`)
    // keeps the connection pool shared instead of opening a second one.
    let dispatch_ctx = (
        client.clone(),
        tenant.bot_token.clone(),
        channel_id.clone(),
        thread_ts.clone(),
        slack_user_id.to_string(),
    );
    let req = SlackRunRequest {
        installation: tenant.installation,
        bot_token: tenant.bot_token.clone(),
        user_link,
        workspace_id,
        agent_path,
        question,
        channel_id,
        thread_ts,
        preagg: preagg.clone(),
    };
    tokio::spawn(async move {
        let (client, bot_token, channel, thread_ts, user) = dispatch_ctx;
        crate::webhooks::events::dispatch_user_event(
            client,
            bot_token,
            channel,
            thread_ts,
            user,
            run_for_slack(req),
        )
        .await;
    });

    Ok(())
}

/// Parse `state.values.workspace_block.workspace_select.selected_option.value` → Uuid.
fn parse_workspace_id_from_state(state_values: Option<&serde_json::Value>) -> Option<Uuid> {
    let raw = state_values?
        .get("workspace_block")?
        .get("workspace_select")?
        .get("selected_option")?
        .get("value")?
        .as_str()?;
    raw.parse::<Uuid>().ok()
}

/// Returns true if the "Set as default for this channel" checkbox was selected.
///
/// `state.values.default_block.set_as_default.selected_options` is an array;
/// it contains `{value: "set_as_default"}` when checked, empty when not.
fn parse_set_as_default_from_state(state_values: Option<&serde_json::Value>) -> bool {
    state_values
        .and_then(|v| v.get("default_block"))
        .and_then(|v| v.get("set_as_default"))
        .and_then(|v| v.get("selected_options"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .any(|opt| opt.get("value").and_then(|v| v.as_str()) == Some("set_as_default"))
        })
        .unwrap_or(false)
}

fn decode_b64_str(encoded: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_workspace_id_from_state() {
        let ws_id = Uuid::new_v4();
        let state_values = json!({
            "workspace_block": {
                "workspace_select": {
                    "type": "static_select",
                    "selected_option": {
                        "text": {"type": "plain_text", "text": "My WS"},
                        "value": ws_id.to_string()
                    }
                }
            }
        });
        let parsed = parse_workspace_id_from_state(Some(&state_values));
        assert_eq!(parsed, Some(ws_id));
    }

    #[test]
    fn returns_none_for_missing_workspace_selection() {
        let state_values = json!({
            "workspace_block": {
                "workspace_select": {
                    "type": "static_select",
                    "selected_option": null
                }
            }
        });
        assert!(parse_workspace_id_from_state(Some(&state_values)).is_none());
    }

    #[test]
    fn parse_set_as_default_checked() {
        let state_values = json!({
            "default_block": {
                "set_as_default": {
                    "type": "checkboxes",
                    "selected_options": [{"value": "set_as_default"}]
                }
            }
        });
        assert!(parse_set_as_default_from_state(Some(&state_values)));
    }

    #[test]
    fn parse_set_as_default_unchecked() {
        let state_values = json!({
            "default_block": {
                "set_as_default": {
                    "type": "checkboxes",
                    "selected_options": []
                }
            }
        });
        assert!(!parse_set_as_default_from_state(Some(&state_values)));
    }

    #[test]
    fn parse_set_as_default_absent() {
        assert!(!parse_set_as_default_from_state(None));
    }

    #[test]
    fn decode_b64_str_roundtrip() {
        use base64::Engine;
        let question = "what is the churn rate?";
        let encoded = base64::engine::general_purpose::STANDARD.encode(question.as_bytes());
        assert_eq!(decode_b64_str(&encoded), Some(question.to_string()));
    }

    #[test]
    fn decode_b64_str_invalid() {
        assert!(decode_b64_str("!!!not-base64!!!").is_none());
    }
}
