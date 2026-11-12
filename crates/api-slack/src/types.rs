use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventPayload {
    UrlVerification(UrlVerification),
    EventCallback(EventCallback),
}

#[derive(Debug, Deserialize)]
pub struct UrlVerification {
    pub challenge: String,
}

#[derive(Debug, Deserialize)]
pub struct EventCallback {
    pub team_id: String,
    #[serde(default)]
    pub enterprise_id: Option<String>,
    #[serde(default)]
    pub api_app_id: Option<String>,
    pub event: Event,
    #[serde(default)]
    pub event_id: Option<String>,
    #[serde(default)]
    pub event_time: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    AppMention {
        user: String,
        text: String,
        ts: String,
        channel: String,
        #[serde(default)]
        thread_ts: Option<String>,
    },
    Message {
        #[serde(default)]
        user: Option<String>,
        #[serde(default)]
        text: Option<String>,
        ts: String,
        channel: String,
        #[serde(default)]
        thread_ts: Option<String>,
        #[serde(default)]
        channel_type: Option<String>,
        #[serde(default)]
        subtype: Option<String>,
        #[serde(default)]
        bot_id: Option<String>,
    },
    AssistantThreadStarted {
        #[serde(default)]
        assistant_thread: Option<serde_json::Value>,
        #[serde(default)]
        user: Option<String>,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        ts: Option<String>,
        #[serde(default)]
        channel: Option<String>,
        #[serde(default)]
        thread_ts: Option<String>,
    },
    AssistantThreadContextChanged {
        #[serde(default)]
        assistant_thread: Option<serde_json::Value>,
        #[serde(default)]
        user: Option<String>,
        #[serde(default)]
        ts: Option<String>,
        #[serde(default)]
        channel: Option<String>,
        #[serde(default)]
        thread_ts: Option<String>,
    },
    AppHomeOpened {
        user: String,
        channel: String,
        tab: String,
        #[serde(default)]
        view: Option<serde_json::Value>,
    },
    AppUninstalled {},
    TokensRevoked {
        #[serde(default)]
        tokens: Option<serde_json::Value>,
    },
}

#[derive(Debug, Deserialize)]
pub struct InteractivityPayload {
    #[serde(rename = "type")]
    pub payload_type: String,
    pub team: InteractivityTeam,
    pub user: InteractivityUser,
    #[serde(default)]
    pub channel: Option<InteractivityChannel>,
    #[serde(default)]
    pub actions: Vec<InteractivityAction>,
    #[serde(default)]
    pub view: Option<serde_json::Value>,
    #[serde(default)]
    pub container: Option<serde_json::Value>,
    /// Form state from `input` blocks — populated by Slack on button clicks.
    /// Shape: `{ values: { <block_id>: { <action_id>: { ... } } } }`
    #[serde(default)]
    pub state: Option<serde_json::Value>,
    #[serde(default)]
    pub trigger_id: Option<String>,
    #[serde(default)]
    pub response_url: Option<String>,
    /// Original message that the action was triggered against. Slack
    /// populates this for `block_actions` payloads with the full
    /// message shape (including `blocks`). Handlers that update via
    /// `response_url` need this to preserve unrelated blocks — sending
    /// `replace_original: true` with only the new block wipes the
    /// prose. Shape: `{ ts, text, blocks: [...], ... }`.
    #[serde(default)]
    pub message: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct InteractivityTeam {
    pub id: String,
}
#[derive(Debug, Deserialize)]
pub struct InteractivityUser {
    pub id: String,
}
#[derive(Debug, Deserialize)]
pub struct InteractivityChannel {
    pub id: String,
}
#[derive(Debug, Clone, Deserialize)]
pub struct InteractivityAction {
    pub action_id: String,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub selected_option: Option<SelectedOption>,
}
#[derive(Debug, Clone, Deserialize)]
pub struct SelectedOption {
    pub value: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── EventPayload deserialization ─────────────────────────────────────────

    #[test]
    fn deserializes_url_verification() {
        let json = r#"{"type":"url_verification","challenge":"abc123"}"#;
        let payload: EventPayload = serde_json::from_str(json).expect("parse");
        match payload {
            EventPayload::UrlVerification(uv) => assert_eq!(uv.challenge, "abc123"),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn deserializes_app_mention_event() {
        let json = r#"{
            "type": "event_callback",
            "team_id": "T12345",
            "event": {
                "type": "app_mention",
                "user": "U111",
                "text": "<@U999> what is MRR?",
                "ts": "1700000001.000000",
                "channel": "C100",
                "thread_ts": "1700000000.000000"
            }
        }"#;
        let payload: EventPayload = serde_json::from_str(json).expect("parse");
        match payload {
            EventPayload::EventCallback(ec) => {
                assert_eq!(ec.team_id, "T12345");
                match ec.event {
                    Event::AppMention {
                        user,
                        text,
                        ts,
                        channel,
                        thread_ts,
                    } => {
                        assert_eq!(user, "U111");
                        assert_eq!(text, "<@U999> what is MRR?");
                        assert_eq!(ts, "1700000001.000000");
                        assert_eq!(channel, "C100");
                        assert_eq!(thread_ts, Some("1700000000.000000".into()));
                    }
                    other => panic!("unexpected event: {other:?}"),
                }
            }
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn deserializes_message_im_event() {
        let json = r#"{
            "type": "event_callback",
            "team_id": "T12345",
            "event": {
                "type": "message",
                "user": "U222",
                "text": "hello bot",
                "ts": "1700000002.000000",
                "channel": "D100",
                "channel_type": "im"
            }
        }"#;
        let payload: EventPayload = serde_json::from_str(json).expect("parse");
        match payload {
            EventPayload::EventCallback(ec) => match ec.event {
                Event::Message {
                    user,
                    text,
                    ts,
                    channel,
                    thread_ts,
                    channel_type,
                    ..
                } => {
                    assert_eq!(user, Some("U222".into()));
                    assert_eq!(text, Some("hello bot".into()));
                    assert_eq!(ts, "1700000002.000000");
                    assert_eq!(channel, "D100");
                    assert_eq!(channel_type, Some("im".into()));
                    assert!(thread_ts.is_none(), "no thread_ts on a fresh DM");
                }
                other => panic!("unexpected event: {other:?}"),
            },
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn deserializes_message_with_thread_reply() {
        let json = r#"{
            "type": "event_callback",
            "team_id": "T12345",
            "event": {
                "type": "message",
                "user": "U333",
                "text": "thread reply",
                "ts": "1700000003.000001",
                "channel": "C200",
                "thread_ts": "1700000003.000000"
            }
        }"#;
        let payload: EventPayload = serde_json::from_str(json).expect("parse");
        match payload {
            EventPayload::EventCallback(ec) => match ec.event {
                Event::Message { ts, thread_ts, .. } => {
                    assert_ne!(ts, thread_ts.as_deref().unwrap_or(""));
                    assert_eq!(thread_ts, Some("1700000003.000000".into()));
                }
                other => panic!("unexpected event: {other:?}"),
            },
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn deserializes_message_without_user() {
        // bot_message subtype often lacks a `user` field
        let json = r#"{
            "type": "event_callback",
            "team_id": "T12345",
            "event": {
                "type": "message",
                "subtype": "bot_message",
                "bot_id": "B001",
                "text": "I am a bot",
                "ts": "1700000004.000000",
                "channel": "C300"
            }
        }"#;
        let payload: EventPayload = serde_json::from_str(json).expect("parse");
        match payload {
            EventPayload::EventCallback(ec) => match ec.event {
                Event::Message {
                    user,
                    text,
                    bot_id,
                    subtype,
                    ..
                } => {
                    assert!(user.is_none(), "no user on bot_message");
                    assert_eq!(text, Some("I am a bot".into()));
                    assert_eq!(bot_id, Some("B001".into()));
                    assert_eq!(subtype, Some("bot_message".into()));
                }
                other => panic!("unexpected event: {other:?}"),
            },
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn deserializes_app_home_opened_event() {
        let json = r#"{
            "type": "event_callback",
            "team_id": "T12345",
            "event": {
                "type": "app_home_opened",
                "user": "U444",
                "channel": "D200",
                "tab": "home"
            }
        }"#;
        let payload: EventPayload = serde_json::from_str(json).expect("parse");
        match payload {
            EventPayload::EventCallback(ec) => match ec.event {
                Event::AppHomeOpened { user, tab, .. } => {
                    assert_eq!(user, "U444");
                    assert_eq!(tab, "home");
                }
                other => panic!("unexpected event: {other:?}"),
            },
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn deserializes_app_uninstalled_event() {
        let json = r#"{
            "type": "event_callback",
            "team_id": "T12345",
            "event": {
                "type": "app_uninstalled"
            }
        }"#;
        let payload: EventPayload = serde_json::from_str(json).expect("parse");
        match payload {
            EventPayload::EventCallback(ec) => match ec.event {
                Event::AppUninstalled {} => {}
                other => panic!("unexpected event: {other:?}"),
            },
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn deserializes_tokens_revoked_event() {
        let json = r#"{
            "type": "event_callback",
            "team_id": "T12345",
            "event": {
                "type": "tokens_revoked",
                "tokens": {"oauth": ["xoxe-123"], "bot": ["xoxb-456"]}
            }
        }"#;
        let payload: EventPayload = serde_json::from_str(json).expect("parse");
        match payload {
            EventPayload::EventCallback(ec) => match ec.event {
                Event::TokensRevoked { tokens } => {
                    assert!(tokens.is_some(), "tokens field should be populated");
                }
                other => panic!("unexpected event: {other:?}"),
            },
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn deserializes_assistant_thread_started() {
        let json = r#"{
            "type": "event_callback",
            "team_id": "T12345",
            "event": {
                "type": "assistant_thread_started",
                "assistant_thread": {"context": {"channel_id": "C100", "team_id": "T12345"}},
                "user": "U555",
                "channel": "D300",
                "ts": "1700000005.000000"
            }
        }"#;
        let payload: EventPayload = serde_json::from_str(json).expect("parse");
        match payload {
            EventPayload::EventCallback(ec) => match ec.event {
                Event::AssistantThreadStarted {
                    assistant_thread,
                    user,
                    channel,
                    ..
                } => {
                    assert!(assistant_thread.is_some(), "assistant_thread should parse");
                    assert_eq!(user, Some("U555".into()));
                    assert_eq!(channel, Some("D300".into()));
                }
                other => panic!("unexpected event: {other:?}"),
            },
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn event_callback_carries_team_id_and_enterprise_id() {
        let json = r#"{
            "type": "event_callback",
            "team_id": "T_TEAM",
            "enterprise_id": "E_ENTERPRISE",
            "api_app_id": "A_APP",
            "event_id": "Ev001",
            "event_time": 1700000000,
            "event": {
                "type": "app_uninstalled"
            }
        }"#;
        let payload: EventPayload = serde_json::from_str(json).expect("parse");
        match payload {
            EventPayload::EventCallback(ec) => {
                assert_eq!(ec.team_id, "T_TEAM");
                assert_eq!(ec.enterprise_id, Some("E_ENTERPRISE".into()));
                assert_eq!(ec.api_app_id, Some("A_APP".into()));
                assert_eq!(ec.event_id, Some("Ev001".into()));
                assert_eq!(ec.event_time, Some(1700000000));
            }
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    // ── InteractivityPayload deserialization ─────────────────────────────────

    #[test]
    fn interactivity_block_actions_parses() {
        let json = r#"{
            "type": "block_actions",
            "team": {"id": "T12345", "domain": "example"},
            "user": {"id": "U666", "username": "alice"},
            "actions": [
                {
                    "action_id": "slack_pick_workspace",
                    "value": "some-value"
                }
            ]
        }"#;
        let payload: InteractivityPayload = serde_json::from_str(json).expect("parse");
        assert_eq!(payload.payload_type, "block_actions");
        assert_eq!(payload.team.id, "T12345");
        assert_eq!(payload.user.id, "U666");
        assert_eq!(payload.actions.len(), 1);
        assert_eq!(payload.actions[0].action_id, "slack_pick_workspace");
        assert_eq!(payload.actions[0].value, Some("some-value".into()));
    }

    #[test]
    fn interactivity_static_select_with_selected_option() {
        let json = r#"{
            "type": "block_actions",
            "team": {"id": "T12345"},
            "user": {"id": "U777"},
            "actions": [
                {
                    "action_id": "slack_pick_workspace",
                    "selected_option": {
                        "text": {"type": "plain_text", "text": "My Workspace"},
                        "value": "some-uuid|aGVsbG8="
                    }
                }
            ]
        }"#;
        let payload: InteractivityPayload = serde_json::from_str(json).expect("parse");
        let action = &payload.actions[0];
        assert_eq!(action.action_id, "slack_pick_workspace");
        let selected = action.selected_option.as_ref().expect("selected_option");
        assert_eq!(selected.value, "some-uuid|aGVsbG8=");
    }
}
