use base64::Engine;

use crate::client::SlackClient;
use crate::events::execution::{SlackRunRequest, run_for_slack};
use crate::resolution::user::{ResolvedUser, resolve as resolve_user};
use crate::resolution::workspace_agent::pick_default_agent_path;
use crate::types::{InteractivityAction, InteractivityPayload};
use crate::webhooks::tenant_resolver;
use oxy_app_core::preagg_cache_ctx::PreaggCacheCtx;
use oxy_shared::errors::OxyError;
use serde_json::Value;
use uuid::Uuid;

/// Handle `slack_pick_workspace` interactivity action.
///
/// Value format: `<workspace_uuid>|<base64_question>` — the workspace UUID
/// the user picked, paired with the original question they asked. We decode
/// the question and dispatch a normal agent run for the chosen workspace.
pub async fn handle(
    payload: &InteractivityPayload,
    action: &InteractivityAction,
    // The node's Layer-1 pre-aggregation cache for the run this dispatches. A
    // default carries no cache, so the agent's semantic queries compile to
    // warehouse SQL — the right answer, just without the rollup shortcut.
    preagg: &PreaggCacheCtx,
) -> Result<(), OxyError> {
    let value = action
        .selected_option
        .as_ref()
        .map(|o| o.value.as_str())
        .unwrap_or_default();

    let Some((workspace_uuid_str, rest)) = value.split_once('|') else {
        tracing::warn!("pick_workspace: malformed action value: {value}");
        return Ok(());
    };

    let workspace_id = match workspace_uuid_str.parse::<Uuid>() {
        Ok(id) => id,
        Err(_) => {
            tracing::warn!("pick_workspace: invalid workspace uuid: {workspace_uuid_str}");
            return Ok(());
        }
    };

    let team_id = &payload.team.id;
    let Some(tenant) = tenant_resolver::resolve(team_id).await? else {
        tracing::warn!("pick_workspace: unknown team {team_id}");
        return Ok(());
    };

    let slack_user_id = &payload.user.id;
    let user_link = match resolve_user(&tenant.installation, slack_user_id).await? {
        ResolvedUser::Linked(link) => link,
        ResolvedUser::Unlinked => {
            tracing::warn!(slack_user_id, "pick_workspace: user not linked");
            return Ok(());
        }
    };

    let Some((channel_id, thread_ts)) = extract_channel_and_thread(payload) else {
        tracing::warn!("pick_workspace: could not extract channel/thread from container");
        return Ok(());
    };

    // Decode question and run agent.
    let question = match decode_question(rest) {
        Some(q) => q,
        None => {
            tracing::warn!("pick_workspace: could not decode question from value");
            return Ok(());
        }
    };

    // Pick the agent: configured `defaults.agent` from config.yml if present,
    // otherwise the alphabetically-first agent in the workspace.
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
                        "text": format!("No agents found in workspace `{workspace_uuid_str}`.")
                    }
                }]),
                "No agents found",
                Some(&thread_ts),
            )
            .await?;
        return Ok(());
    };

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
        if let Err(e) = run_for_slack(req).await {
            tracing::error!("pick_workspace: run_for_slack error: {e}");
        }
    });

    // Delete the workspace-picker message so it doesn't linger after the
    // user has made their selection. The response_url is pre-authenticated
    // by Slack — no bot token required.
    if let Some(url) = &payload.response_url {
        client.delete_via_response_url(url).await;
    }

    Ok(())
}

/// Extract `(channel_id, thread_ts)` from `payload.container` and `payload.channel`.
///
/// `container` is a `serde_json::Value` with keys `channel_id` and `message_ts`.
/// Falls back to `payload.channel.id` for `channel_id`.
pub fn extract_channel_and_thread(payload: &InteractivityPayload) -> Option<(String, String)> {
    let channel_id = payload
        .container
        .as_ref()
        .and_then(|c| c.get("channel_id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| payload.channel.as_ref().map(|c| c.id.clone()))?;

    // Prefer `thread_ts` (the original conversation root) over `message_ts`
    // (the picker's own timestamp). Using the picker's `message_ts` as the
    // thread anchor would start a new sub-thread rooted at the picker message
    // instead of continuing the user's original thread.
    let thread_ts = payload
        .container
        .as_ref()
        .and_then(|c| c.get("thread_ts").or_else(|| c.get("message_ts")))
        .and_then(Value::as_str)
        .map(str::to_owned)?;

    Some((channel_id, thread_ts))
}

fn decode_question(encoded: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    String::from_utf8(bytes).ok()
}

/// Parse a workspace picker action value (`<workspace_uuid>|<base64_question>`).
///
/// Returns `(workspace_id, question)` on success.
pub(crate) fn parse_pick_workspace_value(
    value: &str,
) -> Result<(Uuid, String), oxy_shared::errors::OxyError> {
    let (workspace_uuid_str, encoded_q) = value.split_once('|').ok_or_else(|| {
        oxy_shared::errors::OxyError::ValidationError(
            "workspace picker value must contain a '|' delimiter".into(),
        )
    })?;

    let workspace_id = workspace_uuid_str.parse::<Uuid>().map_err(|_| {
        oxy_shared::errors::OxyError::ValidationError(format!(
            "invalid workspace uuid: {workspace_uuid_str}"
        ))
    })?;

    let question = decode_question(encoded_q).ok_or_else(|| {
        oxy_shared::errors::OxyError::ValidationError(
            "invalid base64 in workspace picker value".into(),
        )
    })?;

    Ok((workspace_id, question))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use serde_json::json;
    use uuid::Uuid;

    // ── parse_pick_workspace_value ───────────────────────────────────────────

    #[test]
    fn parses_valid_workspace_value() {
        let ws_id = Uuid::new_v4();
        let question = "what is the churn rate?";
        let encoded_q = base64::engine::general_purpose::STANDARD.encode(question.as_bytes());
        let raw = format!("{ws_id}|{encoded_q}");

        let (parsed_id, parsed_q) = parse_pick_workspace_value(&raw).expect("parse");
        assert_eq!(parsed_id, ws_id);
        assert_eq!(parsed_q, question);
    }

    #[test]
    fn rejects_workspace_value_missing_delimiter() {
        let err = parse_pick_workspace_value("no-pipe-here").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("'|'") || msg.contains("delimiter"),
            "expected delimiter error, got: {msg}"
        );
    }

    #[test]
    fn rejects_workspace_value_bad_b64() {
        let ws_id = Uuid::new_v4();
        let raw = format!("{ws_id}|!!!");
        let err = parse_pick_workspace_value(&raw).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("base64") || msg.contains("invalid"),
            "expected base64 error, got: {msg}"
        );
    }

    // ── extract_channel_and_thread ───────────────────────────────────────────

    #[test]
    fn extracts_channel_and_thread_from_container() {
        let payload = InteractivityPayload {
            payload_type: "block_actions".into(),
            team: crate::types::InteractivityTeam { id: "T1".into() },
            user: crate::types::InteractivityUser { id: "U1".into() },
            channel: None,
            actions: vec![],
            view: None,
            state: None,
            container: Some(json!({
                "channel_id": "C100",
                "message_ts": "1700000001.000000"
            })),
            trigger_id: None,
            response_url: None,
            message: None,
        };

        let (chan, ts) = extract_channel_and_thread(&payload).expect("should extract");
        assert_eq!(chan, "C100");
        assert_eq!(ts, "1700000001.000000");
    }

    #[test]
    fn falls_back_to_channel_field_when_container_lacks_channel_id() {
        let payload = InteractivityPayload {
            payload_type: "block_actions".into(),
            team: crate::types::InteractivityTeam { id: "T1".into() },
            user: crate::types::InteractivityUser { id: "U1".into() },
            channel: Some(crate::types::InteractivityChannel {
                id: "C_FALLBACK".into(),
            }),
            actions: vec![],
            view: None,
            state: None,
            container: Some(json!({
                "message_ts": "1700000002.000000"
            })),
            trigger_id: None,
            response_url: None,
            message: None,
        };

        let (chan, ts) = extract_channel_and_thread(&payload).expect("should extract");
        assert_eq!(chan, "C_FALLBACK");
        assert_eq!(ts, "1700000002.000000");
    }

    #[test]
    fn returns_none_when_no_thread_ts_available() {
        let payload = InteractivityPayload {
            payload_type: "block_actions".into(),
            team: crate::types::InteractivityTeam { id: "T1".into() },
            user: crate::types::InteractivityUser { id: "U1".into() },
            channel: Some(crate::types::InteractivityChannel { id: "C100".into() }),
            actions: vec![],
            view: None,
            state: None,
            container: None,
            trigger_id: None,
            response_url: None,
            message: None,
        };

        let result = extract_channel_and_thread(&payload);
        assert!(result.is_none(), "should be None when container is absent");
    }
}
