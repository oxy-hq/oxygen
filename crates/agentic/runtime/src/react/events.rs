//! Event rows the react loop writes.
//!
//! Encoding: every row is `(event_type, payload)`, the same shape
//! `agentic_core::events::Event::serialize` produces for `CoreEvent`. The
//! `event_type` tag is split OFF the payload (the payload holds only the
//! variant's fields), so `ReactTurn { turn }` is `("react_turn",
//! {"turn": {...}})`. Task 4's `rebuild` parses rows back by re-inserting the
//! tag before deserializing a [`ReactEvent`].

use agentic_core::events::{CoreEvent, Event};
use agentic_core::react::{EventSink, Turn};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Rows that carry what `CoreEvent` cannot: call ids and the verbatim turn.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "event_type", rename_all = "snake_case")]
pub enum ReactEvent {
    ReactTurn {
        turn: Turn,
    },
    ReactToolResult {
        call_id: String,
        name: String,
        text: String,
        is_error: bool,
        /// The tool's `ToolReply.value`; `Null` for failures and older rows.
        #[serde(default, skip_serializing_if = "Value::is_null")]
        value: Value,
    },
    ReactSuspended {
        call_id: String,
    },
}

/// Every `event_type` tag [`ReactEvent`] writes. `rebuild` skips a `react_*`
/// row whose tag is not listed (a newer writer's event) and rejects a listed
/// tag whose payload fails to parse.
pub(super) const REACT_EVENT_TYPES: &[&str] =
    &["react_turn", "react_tool_result", "react_suspended"];

impl ReactEvent {
    /// The `(event_type, payload)` row for this event (tag split off).
    pub fn to_row(&self) -> (String, Value) {
        split_tag(serde_json::to_value(self).expect("ReactEvent serialization is infallible"))
    }

    /// Inverse of [`to_row`](Self::to_row); `None` for a non-react row.
    pub fn from_row(event_type: &str, payload: &Value) -> Option<Self> {
        let mut v = payload.clone();
        v.as_object_mut()?
            .insert("event_type".into(), event_type.into());
        serde_json::from_value(v).ok()
    }
}

fn split_tag(v: Value) -> (String, Value) {
    let Value::Object(mut obj) = v else {
        unreachable!("internally tagged enum serializes to an object");
    };
    let t = match obj.remove("event_type") {
        Some(Value::String(s)) => s,
        _ => String::new(),
    };
    (t, Value::Object(obj))
}

/// Sends one row; a closed sink is logged, never fatal to the loop.
pub(super) async fn send_row(ev: &EventSink, row: (String, Value)) {
    if ev.send(row).await.is_err() {
        tracing::warn!("react: event sink closed; row dropped");
    }
}

pub(super) async fn emit_core(ev: &EventSink, e: CoreEvent) {
    send_row(ev, Event::<()>::Core(e).serialize()).await;
}

pub(super) async fn emit_react(ev: &EventSink, e: ReactEvent) {
    send_row(ev, e.to_row()).await;
}

/// `react_turn` plus the `LlmEnd` the existing UI transforms read.
pub(super) async fn emit_turn(ev: &EventSink, turn: &Turn) {
    let u = &turn.usage;
    emit_core(
        ev,
        CoreEvent::LlmEnd {
            state: "react".into(),
            output_tokens: u.output_tokens as usize,
            cache_creation_input_tokens: u.cache_write_tokens as usize,
            cache_read_input_tokens: u.cache_read_tokens as usize,
            duration_ms: 0,
            model: u.model.clone(),
            sub_spec_index: None,
        },
    )
    .await;
    emit_react(ev, ReactEvent::ReactTurn { turn: turn.clone() }).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn react_rows_round_trip_with_the_tag_split_off() {
        let e = ReactEvent::ReactToolResult {
            call_id: "c1".into(),
            name: "echo".into(),
            text: "a".into(),
            is_error: false,
            value: serde_json::json!({"result_id": "r1", "sql": "select 1"}),
        };
        let (t, p) = e.to_row();
        assert_eq!(t, "react_tool_result");
        assert!(p.get("event_type").is_none());
        assert_eq!(ReactEvent::from_row(&t, &p), Some(e));
    }

    #[test]
    fn react_event_types_lists_every_variant_tag() {
        let turn = agentic_core::react::Turn {
            text: String::new(),
            tool_calls: vec![],
            native: Value::Null,
            usage: Default::default(),
        };
        let all = [
            ReactEvent::ReactTurn { turn },
            ReactEvent::ReactToolResult {
                call_id: "c".into(),
                name: "n".into(),
                text: "t".into(),
                is_error: false,
                value: Value::Null,
            },
            ReactEvent::ReactSuspended {
                call_id: "c".into(),
            },
        ];
        let tags: Vec<String> = all.iter().map(|e| e.to_row().0).collect();
        assert_eq!(tags, REACT_EVENT_TYPES);
    }

    #[test]
    fn react_tool_result_row_without_a_value_key_still_parses() {
        let p = serde_json::json!({"call_id": "c1", "name": "q", "text": "t", "is_error": false});
        assert!(matches!(
            ReactEvent::from_row("react_tool_result", &p),
            Some(ReactEvent::ReactToolResult {
                value: Value::Null,
                ..
            })
        ));
    }
}
