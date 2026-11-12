use crate::types::{EventCallback, InteractivityPayload};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SocketEvent {
    Hello,
    EventsApi {
        envelope_id: String,
        payload: EventCallback,
    },
    Interactive {
        envelope_id: String,
        payload: InteractivityPayload,
    },
    SlashCommands {
        envelope_id: String,
        #[serde(default)]
        payload: serde_json::Value,
    },
    Disconnect {
        #[serde(default)]
        reason: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_hello() {
        let v: SocketEvent = serde_json::from_str(r#"{"type":"hello"}"#).unwrap();
        assert!(matches!(v, SocketEvent::Hello));
    }

    #[test]
    fn deserializes_events_api_envelope() {
        let json = r#"{
            "envelope_id":"abc-123","type":"events_api",
            "payload":{"team_id":"T1","event":{"type":"app_mention",
                "user":"U1","text":"<@B1> hi","ts":"1.2","channel":"C1"}}
        }"#;
        match serde_json::from_str::<SocketEvent>(json).unwrap() {
            SocketEvent::EventsApi {
                envelope_id,
                payload,
            } => {
                assert_eq!(envelope_id, "abc-123");
                assert_eq!(payload.team_id, "T1");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn deserializes_interactive_envelope() {
        let json = r#"{
            "envelope_id":"int-42","type":"interactive",
            "payload":{"type":"block_actions","team":{"id":"T1"},"user":{"id":"U1"},"actions":[]}
        }"#;
        match serde_json::from_str::<SocketEvent>(json).unwrap() {
            SocketEvent::Interactive { envelope_id, .. } => assert_eq!(envelope_id, "int-42"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn deserializes_disconnect() {
        let v: SocketEvent =
            serde_json::from_str(r#"{"type":"disconnect","reason":"refresh_requested"}"#).unwrap();
        match v {
            SocketEvent::Disconnect { reason } => {
                assert_eq!(reason.as_deref(), Some("refresh_requested"))
            }
            _ => panic!("wrong variant"),
        }
    }
}
