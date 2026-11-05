//! Domain-agnostic event registry for SSE streaming.
//!
//! Domains register a [`RowProcessor`] at startup. The SSE handler looks up
//! the run's `source_type` and uses the corresponding processor to convert raw
//! DB rows `(event_type, payload)` into frontend-ready `(event_type, payload)` pairs.
//!
//! Core events (`CoreEvent` variants) are handled by a built-in processor that
//! uses [`UiTransformState<()>`] from `agentic-core`. Domain-specific events
//! are handled by the registered domain processor.

use std::collections::HashMap;
use std::sync::Arc;

use agentic_core::UiTransformState;
use agentic_core::events::{CoreEvent, Event};
use serde_json::{Value, json};

/// Processes a raw DB row `(event_type, payload)` into zero or more
/// frontend-ready `(event_type, payload)` pairs.
///
/// Returns `None` if the event type is not recognized by this processor.
pub type RowProcessor = Arc<dyn Fn(&str, &Value) -> Option<Vec<(String, Value)>> + Send + Sync>;

/// Summary function: maps a state name to an optional one-line summary.
pub type SummaryFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Tool summary function: maps a tool name and its input params to an optional step summary update.
pub type ToolSummaryFn = Arc<dyn Fn(&str, &serde_json::Value) -> Option<String> + Send + Sync>;

/// Filter that determines whether a domain event should accumulate into
/// `StepEnd` metadata. Receives the `event_type` string.
pub type AccumulationFilter = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Domain registration: a row processor plus optional summary functions.
pub struct DomainHandler {
    /// Processes domain-specific events (not core events).
    pub processor: RowProcessor,
    /// Maps state names to summaries (used by the core processor for StepStart).
    pub summary_fn: SummaryFn,
    /// Maps tool names to step summary updates (used by the core processor for ToolCall).
    pub tool_summary_fn: ToolSummaryFn,
    /// When `Some`, only events matching the filter accumulate into `StepEnd`
    /// metadata. When `None`, all domain events accumulate (legacy behavior).
    pub should_accumulate: Option<AccumulationFilter>,
}

/// Registry of domain event handlers, keyed by `source_type`.
///
/// The SSE handler uses this to convert raw DB rows into frontend events
/// without importing any domain event types.
pub struct EventRegistry {
    domains: HashMap<String, DomainHandler>,
}

impl Default for EventRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl EventRegistry {
    pub fn new() -> Self {
        Self {
            domains: HashMap::new(),
        }
    }

    /// Register a domain handler for the given `source_type`.
    pub fn register(&mut self, source_type: &str, handler: DomainHandler) {
        self.domains.insert(source_type.to_string(), handler);
    }

    /// Create a new [`StreamProcessor`] for a specific run.
    ///
    /// The processor holds per-connection state (`UiTransformState` + metadata
    /// accumulator) and should not be shared across SSE connections.
    pub fn stream_processor(&self, source_type: &str) -> StreamProcessor {
        let domain = self.domains.get(source_type);
        let summary_fn: Box<dyn Fn(&str) -> Option<String> + Send> = if let Some(d) = domain {
            let f = d.summary_fn.clone();
            Box::new(move |s| f(s))
        } else {
            Box::new(|_| None)
        };
        let tool_summary_fn: Box<dyn Fn(&str, &serde_json::Value) -> Option<String> + Send> =
            if let Some(d) = domain {
                let f = d.tool_summary_fn.clone();
                Box::new(move |s, input| f(s, input))
            } else {
                Box::new(|_, _| None)
            };
        let domain_processor = domain.map(|d| d.processor.clone());
        let accumulation_filter = domain.and_then(|d| d.should_accumulate.clone());

        StreamProcessor {
            ui_state: UiTransformState::new()
                .with_summary_fn(summary_fn)
                .with_tool_summary_fn(tool_summary_fn),
            domain_processor,
            accumulation_filter,
            pending_domain: serde_json::Map::new(),
        }
    }
}

/// Per-connection stream processor.
///
/// Created via [`EventRegistry::stream_processor`]. Holds stateful
/// `UiTransformState` and domain-event accumulator for `StepEnd` metadata
/// enrichment.
pub struct StreamProcessor {
    ui_state: UiTransformState<()>,
    domain_processor: Option<RowProcessor>,
    /// Optional filter selecting which domain events accumulate into
    /// `StepEnd` metadata. `None` means accumulate all.
    accumulation_filter: Option<AccumulationFilter>,
    /// Domain-event payloads accumulated since the last `StepStart`, injected
    /// into `StepEnd` as a `metadata` field.
    pending_domain: serde_json::Map<String, Value>,
}

impl StreamProcessor {
    /// Process a raw DB row into zero or more frontend-ready `(event_type, payload)` pairs.
    pub fn process(&mut self, event_type: &str, payload: &Value) -> Vec<(String, Value)> {
        // Try to deserialize as a CoreEvent first.
        if let Some(core) = deserialize_core(event_type, payload) {
            let blocks = self.ui_state.process(Event::Core(core));
            return blocks
                .into_iter()
                .flat_map(|block| self.serialize_ui_block(&block))
                .collect();
        }

        // Try the domain-specific processor.
        if let Some(ref processor) = self.domain_processor
            && let Some(events) = processor(event_type, payload)
        {
            // Accumulate domain events for StepEnd metadata enrichment,
            // filtered by the domain's `should_accumulate` (if provided).
            for (et, p) in &events {
                if self.accumulation_filter.as_ref().is_none_or(|f| f(et)) {
                    self.pending_domain.insert(et.clone(), p.clone());
                }
            }
            return events;
        }

        // Coordinator lifecycle events pass through as-is so the frontend
        // can render delegation cards and other structural UI.
        // (`delegation_retry` / `delegation_fallback` were never routed here
        // and are no longer emitted: the coordinator's per-node retry is gone.
        // Older runs may still hold them in `agentic_run_events`.)
        if matches!(
            event_type,
            "delegation_started"
                | "delegation_completed"
                | "delegation_event"
                | "task_failed"
                | "worker_task_claimed"
                | "waiting_on_children"
                | "decider_decided"
        ) {
            return vec![(event_type.to_string(), payload.clone())];
        }

        // Unrecognized event type — skip.
        vec![]
    }

    /// Serialize a `UiBlock<()>` into `(event_type, payload)` pairs.
    ///
    /// Delegates to `agentic_core::serialize_ui_block` for most variants.
    /// Handles `StepStart` (clear pending domain) and `StepEnd` (inject
    /// accumulated domain-event metadata) specially.
    fn serialize_ui_block(&mut self, block: &agentic_core::UiBlock<()>) -> Vec<(String, Value)> {
        use agentic_core::UiBlock;
        match block {
            UiBlock::StepStart { .. } => {
                self.pending_domain.clear();
                let (et, payload) = agentic_core::serialize_ui_block(block);
                vec![(et, payload)]
            }
            UiBlock::StepEnd {
                label,
                outcome,
                sub_spec_index,
            } => {
                let metadata = if self.pending_domain.is_empty() {
                    Value::Null
                } else {
                    Value::Object(std::mem::take(&mut self.pending_domain))
                };
                vec![(
                    "step_end".into(),
                    json!({ "label": label, "outcome": outcome, "metadata": metadata, "sub_spec_index": sub_spec_index }),
                )]
            }
            other => {
                let (et, payload) = agentic_core::serialize_ui_block(other);
                vec![(et, payload)]
            }
        }
    }
}

// ── CoreEvent deserialization ─────────────────────────────────────────────────

/// Reconstruct a `CoreEvent` from a raw DB row.
///
/// Returns `None` if the event type doesn't match any `CoreEvent` variant.
fn deserialize_core(event_type: &str, payload: &Value) -> Option<CoreEvent> {
    let mut tagged = match payload {
        Value::Object(m) => m.clone(),
        _ => serde_json::Map::new(),
    };
    tagged.insert("event_type".into(), Value::String(event_type.to_string()));
    serde_json::from_value::<CoreEvent>(Value::Object(tagged)).ok()
}

// ── Generic domain row processor ─────────────────────────────────────────────

/// Build a [`RowProcessor`] that handles any domain event enum using serde
/// internally-tagged deserialization.
///
/// The returned closure:
/// 1. Unwraps `delegation_event` envelopes (extracts `inner_event_type` / `inner`)
/// 2. Reconstructs the serde `event_type` tag on the payload
/// 3. Deserializes into `E`
/// 4. Re-serializes and splits the tag back off
///
/// This eliminates the per-domain boilerplate that was previously duplicated
/// in every domain's `event_handler()`.
pub fn domain_row_processor<E>() -> RowProcessor
where
    E: serde::Serialize + serde::de::DeserializeOwned + Send + Sync + 'static,
{
    Arc::new(|event_type, payload| {
        // Unwrap delegation_event envelopes.
        let (effective_type, effective_payload) = if event_type == "delegation_event" {
            let inner_type = payload
                .get("inner_event_type")
                .and_then(|v| v.as_str())
                .unwrap_or(event_type);
            let inner_payload = payload.get("inner").unwrap_or(payload);
            (inner_type.to_string(), inner_payload.clone())
        } else {
            (event_type.to_string(), payload.clone())
        };

        // Reconstruct the internally-tagged object.
        let mut tagged = match &effective_payload {
            Value::Object(m) => m.clone(),
            _ => serde_json::Map::new(),
        };
        tagged.insert("event_type".into(), Value::String(effective_type));

        // Deserialize into the domain enum.
        let event: E = serde_json::from_value(Value::Object(tagged)).ok()?;

        // Re-serialize and split the tag.
        let val = serde_json::to_value(&event).ok()?;
        let Value::Object(mut obj) = val else {
            return None;
        };
        let et = obj
            .remove("event_type")
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        Some(vec![(et, Value::Object(obj))])
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_core_processor_handles_step_enter() {
        let registry = EventRegistry::new();
        let mut proc = registry.stream_processor("analytics");
        let results = proc.process(
            "state_enter",
            &json!({ "state": "clarifying", "revision": 0, "trace_id": "t" }),
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "step_start");
        assert_eq!(results[0].1["label"], "clarifying");
    }

    #[test]
    fn test_core_processor_handles_llm_token() {
        let registry = EventRegistry::new();
        let mut proc = registry.stream_processor("analytics");
        let results = proc.process("llm_token", &json!({ "token": "hello" }));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "text_delta");
        assert_eq!(results[0].1["token"], "hello");
    }

    #[test]
    fn test_core_processor_handles_done() {
        let registry = EventRegistry::new();
        let mut proc = registry.stream_processor("analytics");
        let results = proc.process("done", &json!({ "answer": "42", "trace_id": "t" }));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "done");
    }

    #[test]
    fn test_core_processor_handles_error() {
        let registry = EventRegistry::new();
        let mut proc = registry.stream_processor("analytics");
        let results = proc.process("error", &json!({ "message": "fail", "trace_id": "t" }));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "error");
        assert_eq!(results[0].1["message"], "fail");
    }

    #[test]
    fn test_core_processor_ignores_unknown_events() {
        let registry = EventRegistry::new();
        let mut proc = registry.stream_processor("analytics");
        let results = proc.process("unknown_event", &json!({ "foo": "bar" }));
        assert!(results.is_empty());
    }

    #[test]
    fn test_domain_processor_called_for_registered_source_type() {
        let mut registry = EventRegistry::new();
        registry.register(
            "test_domain",
            DomainHandler {
                processor: Arc::new(|event_type, _payload| {
                    if event_type == "custom_event" {
                        Some(vec![("custom_ui".into(), json!({ "data": "processed" }))])
                    } else {
                        None
                    }
                }),
                summary_fn: Arc::new(|_| None),
                tool_summary_fn: Arc::new(|_, _| None),
                should_accumulate: None,
            },
        );
        let mut proc = registry.stream_processor("test_domain");
        let results = proc.process("custom_event", &json!({ "data": "raw" }));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "custom_ui");
        assert_eq!(results[0].1["data"], "processed");
    }

    #[test]
    fn test_fallback_to_core_when_domain_returns_none() {
        let mut registry = EventRegistry::new();
        registry.register(
            "test_domain",
            DomainHandler {
                processor: Arc::new(|_, _| None), // always returns None
                summary_fn: Arc::new(|_| None),
                tool_summary_fn: Arc::new(|_, _| None),
                should_accumulate: None,
            },
        );
        let mut proc = registry.stream_processor("test_domain");
        // Core event should still work even though domain processor returns None.
        let results = proc.process("done", &json!({ "answer": "ok", "trace_id": "t" }));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "done");
    }

    #[test]
    fn test_registry_with_no_domain_processor() {
        let registry = EventRegistry::new();
        // Unknown source_type — no domain processor registered.
        let mut proc = registry.stream_processor("nonexistent");
        // Core events still work.
        let results = proc.process(
            "llm_token",
            &json!({ "token": "hi", "sub_spec_index": null }),
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "text_delta");
    }

    #[test]
    fn test_summary_fn_used_in_step_start() {
        let mut registry = EventRegistry::new();
        registry.register(
            "test_domain",
            DomainHandler {
                processor: Arc::new(|_, _| None),
                summary_fn: Arc::new(|state| {
                    if state == "clarifying" {
                        Some("Understanding your question".into())
                    } else {
                        None
                    }
                }),
                tool_summary_fn: Arc::new(|_, _| None),
                should_accumulate: None,
            },
        );
        let mut proc = registry.stream_processor("test_domain");
        let results = proc.process(
            "state_enter",
            &json!({ "state": "clarifying", "revision": 0, "trace_id": "t" }),
        );
        assert_eq!(results[0].1["summary"], "Understanding your question");
    }

    #[test]
    fn test_step_end_includes_accumulated_domain_metadata() {
        let mut registry = EventRegistry::new();
        registry.register(
            "test_domain",
            DomainHandler {
                processor: Arc::new(|event_type, payload| {
                    if event_type == "custom" {
                        Some(vec![("custom".into(), payload.clone())])
                    } else {
                        None
                    }
                }),
                summary_fn: Arc::new(|_| None),
                tool_summary_fn: Arc::new(|_, _| None),
                should_accumulate: None,
            },
        );
        let mut proc = registry.stream_processor("test_domain");

        // StepStart
        proc.process(
            "state_enter",
            &json!({ "state": "s", "revision": 0, "trace_id": "t" }),
        );
        // Domain event — accumulated
        proc.process("custom", &json!({ "key": "value" }));
        // StepEnd — should include metadata
        let results = proc.process(
            "state_exit",
            &json!({ "state": "s", "outcome": "advanced", "trace_id": "t" }),
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "step_end");
        assert_eq!(results[0].1["metadata"]["custom"]["key"], "value");
    }

    #[test]
    fn test_accumulation_filter_excludes_non_matching_events() {
        let mut registry = EventRegistry::new();
        registry.register(
            "test_domain",
            DomainHandler {
                processor: Arc::new(|event_type, payload| {
                    Some(vec![(event_type.to_string(), payload.clone())])
                }),
                summary_fn: Arc::new(|_| None),
                tool_summary_fn: Arc::new(|_, _| None),
                // Only `keep_me` events accumulate.
                should_accumulate: Some(Arc::new(|et| et == "keep_me")),
            },
        );
        let mut proc = registry.stream_processor("test_domain");

        // StepStart
        proc.process(
            "state_enter",
            &json!({ "state": "s", "revision": 0, "trace_id": "t" }),
        );
        // This one should be filtered out.
        proc.process("drop_me", &json!({ "a": 1 }));
        // This one should accumulate.
        proc.process("keep_me", &json!({ "b": 2 }));
        // StepEnd
        let results = proc.process(
            "state_exit",
            &json!({ "state": "s", "outcome": "advanced", "trace_id": "t" }),
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "step_end");
        assert_eq!(results[0].1["metadata"]["keep_me"]["b"], 2);
        assert!(results[0].1["metadata"].get("drop_me").is_none());
    }

    // ── domain_row_processor tests ──────────────────────────────────────────

    #[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq)]
    #[serde(tag = "event_type", rename_all = "snake_case")]
    enum TestDomainEvent {
        Foo { x: i32 },
        Bar { y: String },
    }

    #[test]
    fn test_domain_row_processor_round_trip() {
        let processor = domain_row_processor::<TestDomainEvent>();
        let result = processor("foo", &json!({ "x": 42 }));
        assert_eq!(result, Some(vec![("foo".into(), json!({ "x": 42 }))]));
    }

    #[test]
    fn test_domain_row_processor_delegation_unwrap() {
        let processor = domain_row_processor::<TestDomainEvent>();
        let payload = json!({
            "child_task_id": "child-1",
            "inner_event_type": "bar",
            "inner": { "y": "hello" }
        });
        let result = processor("delegation_event", &payload);
        assert_eq!(result, Some(vec![("bar".into(), json!({ "y": "hello" }))]));
    }

    #[test]
    fn test_domain_row_processor_unknown_returns_none() {
        let processor = domain_row_processor::<TestDomainEvent>();
        let result = processor("unknown_event", &json!({ "x": 1 }));
        assert_eq!(result, None);
    }
}
