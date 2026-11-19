//! The run's event log is the transcript: [`rebuild`] folds `react_*` rows
//! back into a [`Transcript`]. Only `react_turn` and `react_tool_result` rows
//! count; `CoreEvent` rows (UI events) and unknown types are skipped, an
//! unknown `react_*` tag (a newer writer's event) with a warning.

use std::collections::HashMap;

use agentic_core::react::{Entry, ResultHandle, ToolCallReq, Transcript};
use async_trait::async_trait;
use serde_json::Value;

use super::events::{REACT_EVENT_TYPES, ReactEvent};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("event store error: {0}")]
    Db(String),
    #[error("malformed `{event_type}` row: {reason}")]
    Malformed { event_type: String, reason: String },
}

/// Where a run's rows come from.
#[async_trait]
pub trait TranscriptStore: Send + Sync {
    /// `(event_type, payload)` rows of `run_id`, in seq order.
    async fn load(&self, run_id: &str) -> Result<Vec<(String, Value)>, StoreError>;
}

/// Rows held in memory, e.g. what an `EventSink` collected.
pub struct InMemoryStore {
    rows: Vec<(String, Value)>,
}

impl InMemoryStore {
    pub fn new(rows: Vec<(String, Value)>) -> Self {
        Self { rows }
    }
}

#[async_trait]
impl TranscriptStore for InMemoryStore {
    async fn load(&self, _run_id: &str) -> Result<Vec<(String, Value)>, StoreError> {
        Ok(self.rows.clone())
    }
}

/// A transcript rebuilt from the log.
#[derive(Debug)]
pub struct Rebuilt {
    /// `seed` plus every recorded turn (the final reply included) and result.
    pub transcript: Transcript,
    /// Recorded tool results by call id; these calls are never re-run.
    pub recorded: HashMap<String, Entry>,
    /// The last turn's calls that have no recorded result.
    pub pending_calls: Vec<ToolCallReq>,
}

/// Folds `rows` onto `seed`. A known `react_*` row whose payload does not
/// parse is an error; an unknown `react_*` tag is skipped. A second
/// `react_tool_result` for a call id already recorded is ignored (the first
/// wins). With `results`, every recorded `{result_id, sql}` value is restored
/// into the store so the ids stay known (their rows are not).
pub fn rebuild(
    seed: Transcript,
    rows: &[(String, Value)],
    results: Option<&dyn ResultHandle>,
) -> Result<Rebuilt, StoreError> {
    let mut transcript = seed;
    let mut recorded = HashMap::new();
    let mut last_calls: Vec<ToolCallReq> = Vec::new();
    for (event_type, payload) in rows {
        let Some(ev) = parse_row(event_type, payload)? else {
            continue;
        };
        match ev {
            ReactEvent::ReactTurn { turn } => {
                last_calls = turn.tool_calls.clone();
                transcript.entries.push(Entry::Assistant(turn));
            }
            ReactEvent::ReactToolResult {
                call_id,
                text,
                is_error,
                value,
                ..
            } => {
                if recorded.contains_key(&call_id) {
                    tracing::warn!(%call_id, "react: duplicate tool result ignored");
                    continue;
                }
                if let Some(store) = results {
                    restore_from_value(store, &value);
                }
                let entry = Entry::ToolResult {
                    call_id: call_id.clone(),
                    text,
                    is_error,
                };
                transcript.entries.push(entry.clone());
                recorded.insert(call_id, entry);
            }
            ReactEvent::ReactSuspended { .. } => {}
        }
    }
    last_calls.retain(|c| !recorded.contains_key(&c.id));
    Ok(Rebuilt {
        transcript,
        recorded,
        pending_calls: last_calls,
    })
}

/// `None` for a row `rebuild` skips; `Malformed` for a known react tag whose
/// payload does not match the schema.
fn parse_row(event_type: &str, payload: &Value) -> Result<Option<ReactEvent>, StoreError> {
    if !event_type.starts_with("react_") {
        return Ok(None);
    }
    if !REACT_EVENT_TYPES.contains(&event_type) {
        tracing::warn!(event_type, "react: unknown react event skipped");
        return Ok(None);
    }
    ReactEvent::from_row(event_type, payload)
        .map(Some)
        .ok_or_else(|| StoreError::Malformed {
            event_type: event_type.to_string(),
            reason: "payload does not match the event schema".into(),
        })
}

fn restore_from_value(store: &dyn ResultHandle, value: &Value) {
    if let (Some(id), Some(sql)) = (
        value.get("result_id").and_then(Value::as_str),
        value.get("sql").and_then(Value::as_str),
    ) {
        store.restore(id, sql.to_string());
    }
}
