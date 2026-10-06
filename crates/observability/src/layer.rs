use std::collections::HashMap;
use std::time::Instant;

use tokio::sync::mpsc;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;
use uuid::Uuid;

use crate::scope::{WORKSPACE_ATTRIBUTE, stamped_workspace};
use crate::types::SpanRecord;

/// Internal data attached to each span via extensions.
struct SpanData {
    trace_id: String,
    span_id: String,
    start: Instant,
    start_wall: chrono::DateTime<chrono::Utc>,
    attributes: HashMap<String, String>,
    events: Vec<EventRecord>,
    service_name: String,
    has_error_event: bool,
    /// The workspace this span's trace belongs to; `""` until something says.
    /// Decided once, at the top: a root that names a workspace hands it to
    /// everything beneath it, and a span whose parent already has one cannot
    /// take another — a trace is one tenant's or nobody's, never split.
    workspace_id: String,
}

/// The workspace `attributes` claim, if they claim one.
fn workspace_of(attributes: &HashMap<String, String>) -> String {
    attributes
        .get(WORKSPACE_ATTRIBUTE)
        .and_then(|raw| stamped_workspace(raw))
        .unwrap_or_default()
}

/// A single event captured within a span.
struct EventRecord {
    name: String,
    attributes: HashMap<String, String>,
}

/// Generate a 32-character hex trace ID from a UUID v4.
fn new_trace_id() -> String {
    let uuid = Uuid::new_v4();
    let bytes = uuid.as_bytes();
    hex::encode(bytes)
}

/// Generate a 16-character hex span ID from the first 8 bytes of a UUID v4.
fn new_span_id() -> String {
    let uuid = Uuid::new_v4();
    let bytes = uuid.as_bytes();
    hex::encode(&bytes[..8])
}

// Visitors for extracting field values from spans and events

/// Visitor that collects all fields into a HashMap<String, String>.
struct FieldVisitor {
    fields: HashMap<String, String>,
}

impl FieldVisitor {
    fn new() -> Self {
        Self {
            fields: HashMap::new(),
        }
    }
}

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.fields
            .insert(field.name().to_string(), format!("{:?}", value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }
}

struct EventVisitor {
    name: Option<String>,
    fields: HashMap<String, String>,
}

impl EventVisitor {
    fn new() -> Self {
        Self {
            name: None,
            fields: HashMap::new(),
        }
    }
}

impl Visit for EventVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let key = field.name();
        let val = format!("{:?}", value);
        if key == "name" {
            self.name = Some(val.trim_matches('"').to_string());
        } else {
            self.fields.insert(key.to_string(), val);
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        let key = field.name();
        if key == "name" {
            self.name = Some(value.to_string());
        } else {
            self.fields.insert(key.to_string(), value.to_string());
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }
}

// SpanCollectorLayer

/// A custom `tracing_subscriber::Layer` that captures span lifecycle events
/// and sends completed `SpanRecord`s to the observability store via an unbounded channel.
pub struct SpanCollectorLayer {
    sender: mpsc::UnboundedSender<SpanRecord>,
    service_name: String,
}

impl SpanCollectorLayer {
    /// Create a new `SpanCollectorLayer`.
    ///
    /// - `sender`: channel sender for completed span records.
    /// - `service_name`: the service name to attach to every span record.
    pub fn new(sender: mpsc::UnboundedSender<SpanRecord>, service_name: String) -> Self {
        Self {
            sender,
            service_name,
        }
    }
}

impl<S> Layer<S> for SpanCollectorLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let span = match ctx.span(id) {
            Some(s) => s,
            None => return,
        };

        // Inherit the trace — its id and its workspace — from the parent, or
        // start a new one that belongs to nobody yet.
        let inherited = span.parent().and_then(|parent| {
            let extensions = parent.extensions();
            extensions
                .get::<SpanData>()
                .map(|d| (d.trace_id.clone(), d.workspace_id.clone()))
        });
        let (trace_id, inherited_workspace) =
            inherited.unwrap_or_else(|| (new_trace_id(), String::new()));

        // Collect initial span attributes.
        let mut visitor = FieldVisitor::new();
        attrs.record(&mut visitor);

        let workspace_id = if inherited_workspace.is_empty() {
            workspace_of(&visitor.fields)
        } else {
            inherited_workspace
        };

        let data = SpanData {
            trace_id,
            workspace_id,
            span_id: new_span_id(),
            start: Instant::now(),
            start_wall: chrono::Utc::now(),
            attributes: visitor.fields,
            events: Vec::new(),
            service_name: self.service_name.clone(),
            has_error_event: false,
        };

        span.extensions_mut().insert(data);
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let span = match ctx.span(id) {
            Some(s) => s,
            None => return,
        };

        let mut visitor = FieldVisitor::new();
        values.record(&mut visitor);

        let mut extensions = span.extensions_mut();
        if let Some(data) = extensions.get_mut::<SpanData>() {
            data.attributes.extend(visitor.fields);
            // A span that declared the field `Empty` and filled it in later.
            // Children opened before this line were handed nothing, which is
            // why a root should name its workspace when it is created.
            if data.workspace_id.is_empty() {
                data.workspace_id = workspace_of(&data.attributes);
            }
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        // Attach the event to the current span, if any.
        let span = match ctx.current_span().id().and_then(|id| ctx.span(id)) {
            Some(s) => s,
            None => return,
        };

        let mut visitor = EventVisitor::new();
        event.record(&mut visitor);

        // Determine the event name: prefer the explicit "name" field, fall back to metadata name.
        let event_name = visitor
            .name
            .unwrap_or_else(|| event.metadata().name().to_string());

        let is_error = *event.metadata().level() == Level::ERROR;

        let record = EventRecord {
            name: event_name,
            attributes: visitor.fields,
        };

        let mut extensions = span.extensions_mut();
        if let Some(data) = extensions.get_mut::<SpanData>() {
            data.events.push(record);
            if is_error {
                data.has_error_event = true;
            }
        }
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let span = match ctx.span(&id) {
            Some(s) => s,
            None => return,
        };

        // Determine parent_span_id before borrowing extensions.
        let parent_span_id = span
            .parent()
            .and_then(|p| {
                let ext = p.extensions();
                ext.get::<SpanData>().map(|d| d.span_id.clone())
            })
            .unwrap_or_default();

        let mut extensions = span.extensions_mut();
        let data = match extensions.remove::<SpanData>() {
            Some(d) => d,
            None => return,
        };

        let duration_ns = data.start.elapsed().as_nanos() as i64;

        let status_code = if data.has_error_event {
            "ERROR".to_string()
        } else {
            "OK".to_string()
        };

        // Build status_message from the first error event, if any.
        let status_message = if data.has_error_event {
            data.events
                .iter()
                .find(|e| {
                    e.attributes.contains_key("message")
                        || e.attributes.contains_key("error")
                        || e.attributes.contains_key("error.message")
                })
                .and_then(|e| {
                    e.attributes
                        .get("message")
                        .or_else(|| e.attributes.get("error"))
                        .or_else(|| e.attributes.get("error.message"))
                        .cloned()
                })
                .unwrap_or_default()
        } else {
            String::new()
        };

        let span_attributes =
            serde_json::to_string(&data.attributes).unwrap_or_else(|_| "{}".to_string());

        let event_data: Vec<serde_json::Value> = data
            .events
            .iter()
            .map(|e| {
                serde_json::json!({
                    "name": e.name,
                    "attributes": e.attributes,
                })
            })
            .collect();
        let event_data_str =
            serde_json::to_string(&event_data).unwrap_or_else(|_| "[]".to_string());

        // Use the `oxy.name` attribute as span name if present (set by
        // #[tracing::instrument(fields(oxy.name = "..."))]).
        // Otherwise fall back to the tracing metadata name (function name).
        let span_name = data
            .attributes
            .get("oxy.name")
            .cloned()
            .unwrap_or_else(|| span.name().to_string());

        let record = SpanRecord {
            trace_id: data.trace_id,
            span_id: data.span_id,
            parent_span_id,
            span_name,
            service_name: data.service_name,
            span_attributes,
            duration_ns,
            status_code,
            status_message,
            event_data: event_data_str,
            timestamp: data.start_wall.to_rfc3339(),
            workspace_id: data.workspace_id,
        };

        // Send to writer; ignore errors (receiver dropped means shutdown).
        let _ = self.sender.send(record);
    }
}

// Public helper

/// Extract the `trace_id` from the current span's extensions.
///
/// Walks up the span hierarchy starting from `tracing::Span::current()`.
/// Returns `None` if there is no active span or the span was not created
/// by a `SpanCollectorLayer`.
pub fn current_trace_id() -> Option<String> {
    let span = tracing::Span::current();
    span.with_subscriber(|(id, subscriber)| {
        let registry = subscriber.downcast_ref::<tracing_subscriber::Registry>()?;

        // scope() yields an iterator from the current span upward through all ancestors.
        let span_ref = registry.span(id)?;
        for ancestor in span_ref.scope() {
            let extensions = ancestor.extensions();
            if let Some(data) = extensions.get::<SpanData>() {
                return Some(data.trace_id.clone());
            }
        }
        None
    })?
}

#[cfg(test)]
mod workspace_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;
    use tracing::Instrument as _;
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn test_trace_id_format() {
        let tid = new_trace_id();
        assert_eq!(tid.len(), 32);
        assert!(tid.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_span_id_format() {
        let sid = new_span_id();
        assert_eq!(sid.len(), 16);
        assert!(sid.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_ids_are_unique() {
        let ids: Vec<String> = (0..100).map(|_| new_trace_id()).collect();
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(ids.len(), unique.len());
    }

    #[tokio::test]
    async fn test_span_record_sent_on_close() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let layer = SpanCollectorLayer::new(tx, "test-service".to_string());

        let subscriber = tracing_subscriber::registry().with(layer);
        let _guard = tracing::subscriber::set_default(subscriber);

        {
            let span = tracing::info_span!("test_span", key = "value");
            let _enter = span.enter();
            tracing::info!(name = "test.event", detail = "hello");
        }

        let record = rx.try_recv().expect("should receive a SpanRecord");
        assert_eq!(record.span_name, "test_span");
        assert_eq!(record.service_name, "test-service");
        assert_eq!(record.status_code, "OK");
        assert_eq!(record.trace_id.len(), 32);
        assert_eq!(record.span_id.len(), 16);
        assert!(record.parent_span_id.is_empty());
        assert!(record.duration_ns >= 0);

        // Verify attributes contain the span field.
        let attrs: HashMap<String, String> = serde_json::from_str(&record.span_attributes).unwrap();
        assert_eq!(attrs.get("key").map(|s| s.as_str()), Some("value"));

        let events: Vec<serde_json::Value> = serde_json::from_str(&record.event_data).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["name"], "test.event");
    }

    #[tokio::test]
    async fn test_parent_child_trace_id_inheritance() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let layer = SpanCollectorLayer::new(tx, "test-service".to_string());

        let subscriber = tracing_subscriber::registry().with(layer);
        let _guard = tracing::subscriber::set_default(subscriber);

        {
            let parent = tracing::info_span!("parent_span");
            let _parent_enter = parent.enter();
            {
                let child = tracing::info_span!("child_span");
                let _child_enter = child.enter();
            }
        }

        // Child closes first, then parent.
        let child_record = rx.try_recv().expect("child record");
        let parent_record = rx.try_recv().expect("parent record");

        assert_eq!(child_record.span_name, "child_span");
        assert_eq!(parent_record.span_name, "parent_span");
        assert_eq!(child_record.trace_id, parent_record.trace_id);
        assert_eq!(child_record.parent_span_id, parent_record.span_id);
    }

    /// Mirror the exact span shape emitted by `agentic_analytics` for
    /// `analytics.run` + child `analytics.tool_call` spans, and assert that
    /// every attribute consumed by the Execution Analytics SQL query is
    /// faithfully captured — in particular that dotted field names like
    /// `oxy.span_type`, `oxy.execution_type`, `oxy.is_verified`, and
    /// `oxy.agent.ref` round-trip through `FieldVisitor` intact.
    #[tokio::test]
    async fn test_analytics_run_and_tool_call_span_attributes() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let layer = SpanCollectorLayer::new(tx, "test-service".to_string());
        let subscriber = tracing_subscriber::registry().with(layer);
        let _guard = tracing::subscriber::set_default(subscriber);

        {
            let run = tracing::info_span!(
                "analytics.run",
                oxy.name = "analytics.run",
                oxy.span_type = "analytics",
                oxy.agent.ref = "revenue_agent",
                agent.prompt = "top customers",
                question = "top customers",
            );
            let _run_enter = run.enter();
            {
                let tool = tracing::info_span!(
                    "analytics.tool_call",
                    oxy.name = "analytics.tool_call",
                    oxy.span_type = "tool_call",
                    oxy.execution_type = "semantic_query",
                    oxy.is_verified = true,
                    connector = "duckdb",
                );
                let _tool_enter = tool.enter();
                tracing::info!(name: "tool_call.output", status = "success", row_count = 7);
            }
        }

        let tool_record = rx.try_recv().expect("tool record");
        let run_record = rx.try_recv().expect("run record");

        assert_eq!(tool_record.span_name, "analytics.tool_call");
        assert_eq!(run_record.span_name, "analytics.run");
        assert_eq!(tool_record.trace_id, run_record.trace_id);
        assert_eq!(tool_record.parent_span_id, run_record.span_id);

        let tool_attrs: HashMap<String, String> =
            serde_json::from_str(&tool_record.span_attributes).unwrap();
        assert_eq!(
            tool_attrs.get("oxy.span_type").map(String::as_str),
            Some("tool_call")
        );
        assert_eq!(
            tool_attrs.get("oxy.execution_type").map(String::as_str),
            Some("semantic_query")
        );
        assert_eq!(
            tool_attrs.get("oxy.is_verified").map(String::as_str),
            Some("true")
        );

        let run_attrs: HashMap<String, String> =
            serde_json::from_str(&run_record.span_attributes).unwrap();
        assert_eq!(
            run_attrs.get("oxy.span_type").map(String::as_str),
            Some("analytics")
        );
        assert_eq!(
            run_attrs.get("oxy.agent.ref").map(String::as_str),
            Some("revenue_agent")
        );
        assert_eq!(
            run_attrs.get("agent.prompt").map(String::as_str),
            Some("top customers")
        );

        // The `tool_call.output` event must land on the tool span.
        let events: Vec<serde_json::Value> = serde_json::from_str(&tool_record.event_data).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["name"], "tool_call.output");
        assert_eq!(events[0]["attributes"]["status"], "success");
    }

    /// Exercise the exact async_trait path the fanout worker uses and confirm
    /// that an inner `info_span!` inherits the outer `.instrument(sub_span)`
    /// chain as its parent — so trace_id inheritance works.
    #[tokio::test(flavor = "current_thread")]
    async fn test_info_span_parent_through_async_trait_and_spawn() {
        use std::sync::Arc;
        use tracing::Instrument as _;
        use tracing_subscriber::layer::SubscriberExt;

        #[async_trait::async_trait]
        trait Worker: Send + Sync {
            async fn go(&self) -> String;
        }
        struct W;
        #[async_trait::async_trait]
        impl Worker for W {
            async fn go(&self) -> String {
                async { /* connector.execute_query */ }.await;
                let tool_span = tracing::info_span!("analytics.tool_call");
                tool_span.in_scope(|| current_trace_id().unwrap_or_default())
            }
        }

        let (tx, _rx) = mpsc::unbounded_channel();
        let layer = SpanCollectorLayer::new(tx, "oxy".to_string());
        let subscriber = tracing_subscriber::registry().with(layer);
        let _default = tracing::subscriber::set_default(subscriber);

        let run_span = tracing::info_span!(parent: None, "analytics.run");
        let sub_span = tracing::info_span!(parent: &run_span, "fanout.sub_spec");

        let w: Arc<dyn Worker> = Arc::new(W);
        let got = tokio::spawn(async move { w.go().await }.instrument(sub_span))
            .await
            .unwrap();

        assert!(
            !got.is_empty(),
            "current_trace_id() must not be empty inside the fanout worker's \
             tool_call span — got {got:?}"
        );
    }

    /// Reproduce the production bug: the FMT log in the bug report shows the
    /// `tool_call.output` event inside `analytics.tool_call` *only* — no
    /// `fanout.sub_spec` or `analytics.run` ancestors visible. That means the
    /// span chain seen by `Span::current()` inside the fanout worker does
    /// *not* reach `analytics.run`, so `current_trace_id()` fails. This test
    /// forces the issue: when `info_span!` is used for the `tool_call` span
    /// with `parent: None` (or without a properly entered parent stack), the
    /// span is rooted at itself rather than under the intended parent.
    #[tokio::test]
    async fn test_fanout_tool_call_span_chain_reaches_root() {
        use tracing::Instrument as _;
        use tracing_subscriber::layer::SubscriberExt;

        let (tx, mut rx) = mpsc::unbounded_channel();
        let layer = SpanCollectorLayer::new(tx, "oxy".to_string());
        let subscriber = tracing_subscriber::registry().with(layer);
        let _default = tracing::subscriber::set_default(subscriber);

        let run_span = tracing::info_span!(
            parent: None,
            "analytics.run",
            oxy.name = "analytics.run",
            oxy.agent.ref = "a",
        );
        let sub_span = tracing::info_span!(parent: &run_span, "fanout.sub_spec");

        let captured_trace_id = tokio::spawn(
            async {
                // Mirror fanout_worker.rs exactly — create tool_span here.
                let tool_span =
                    tracing::info_span!("analytics.tool_call", oxy.span_type = "tool_call",);
                async { /* execute_query */ }
                    .instrument(tool_span.clone())
                    .await;
                tool_span.in_scope(|| {
                    tracing::info!(name: "tool_call.output", status = "success");
                });
                drop(tool_span);
                current_trace_id()
            }
            .instrument(sub_span),
        )
        .await
        .unwrap();

        drop(run_span);

        // Collect all the records, find tool_call and run, verify they share trace_id.
        let mut records = Vec::new();
        while let Ok(r) = rx.try_recv() {
            records.push(r);
        }
        let tool = records
            .iter()
            .find(|r| r.span_name == "analytics.tool_call")
            .expect("tool_call span must be captured");
        let run = records
            .iter()
            .find(|r| r.span_name == "analytics.run")
            .expect("run span must be captured");

        assert_eq!(
            tool.trace_id, run.trace_id,
            "tool_call span must share trace_id with analytics.run"
        );
        assert_eq!(
            captured_trace_id.as_deref(),
            Some(run.trace_id.as_str()),
            "current_trace_id() inside the fanout task must return the \
             analytics.run trace_id"
        );
    }

    /// Exact production pattern: `tokio::spawn(future.instrument(sub_span))`,
    /// where the spawned future runs `await .instrument(tool_span)` then
    /// `in_scope`, then calls `current_trace_id()` synchronously.
    /// This is what the fanout worker does.
    #[tokio::test]
    async fn test_current_trace_id_after_instrument_await_in_spawned_task() {
        use tracing::Instrument as _;
        use tracing_subscriber::layer::SubscriberExt;

        let (tx, _rx) = mpsc::unbounded_channel();
        let layer = SpanCollectorLayer::new(tx, "oxy".to_string());
        let subscriber = std::sync::Arc::new(tracing_subscriber::registry().with(layer));

        // Use set_default (not global set) so the test is self-contained.
        let _default = tracing::subscriber::set_default(subscriber);

        let run_span = tracing::info_span!(parent: None, "analytics.run");
        let sub_span = tracing::info_span!(parent: &run_span, "fanout.sub_spec");

        // Spawn a task instrumented with sub_span (mirrors run_fanout_concurrent).
        // WARNING: `set_default` is NOT propagated to tokio::spawn tasks —
        // only `set_global_default` works across tasks. So this test's setup
        // intentionally uses the current-thread runtime so `set_default`
        // carries into the spawned task's thread.
        let handle = tokio::spawn(
            async {
                let tool_span = tracing::info_span!("analytics.tool_call");
                async { /* connector.execute_query */ }
                    .instrument(tool_span.clone())
                    .await;

                tool_span.in_scope(|| {
                    tracing::info!(name: "tool_call.output", status = "success");
                });

                // The critical call — identical to what metrics_recorder does.
                current_trace_id()
            }
            .instrument(sub_span),
        );

        let got = handle.await.unwrap();
        assert!(
            got.is_some() && !got.as_deref().unwrap_or("").is_empty(),
            "current_trace_id() must return Some after in_scope in an \
             instrumented spawned task; got {got:?}"
        );
    }

    /// Repro for the production bug: `current_trace_id()` returns `None`
    /// when the subscriber is built from multiple layers (fmt + Sentry +
    /// SpanCollectorLayer + EnvFilter) because `downcast_ref::<Registry>()`
    /// on a `Layered<_, _>` chain may not always descend to the bottom.
    #[tokio::test]
    async fn test_current_trace_id_through_layered_subscriber() {
        use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt};

        let (tx, _rx) = mpsc::unbounded_channel();
        let layer = SpanCollectorLayer::new(tx, "oxy".to_string());
        // Mirror the production stack: Registry + EnvFilter + fmt + collector.
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new("debug"))
            .with(fmt::layer().with_writer(std::io::sink))
            .with(layer);
        let _default = tracing::subscriber::set_default(subscriber);

        let root = tracing::info_span!(parent: None, "analytics.run");
        let _enter = root.enter();

        let got = current_trace_id();
        assert!(
            got.is_some() && !got.as_deref().unwrap_or("").is_empty(),
            "current_trace_id() must succeed through a Layered subscriber \
             (got {got:?})"
        );
    }

    /// Reproduce the exact pattern used by `execute_solution` in the
    /// analytics crate: create a `tool_span`, `.instrument()` an awaited
    /// future with a clone, then use `.in_scope()` on the original to emit
    /// the `tool_call.output` event. This asserts that the event still
    /// lands on the tool span after the instrumented future has completed.
    #[tokio::test]
    async fn test_tool_call_output_event_after_instrumented_await() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let layer = SpanCollectorLayer::new(tx, "test-service".to_string());
        let subscriber = tracing_subscriber::registry().with(layer);
        let _guard = tracing::subscriber::set_default(subscriber);

        {
            let run = tracing::info_span!(
                "analytics.run",
                oxy.name = "analytics.run",
                oxy.span_type = "analytics",
                oxy.agent.ref = "test_agent",
                agent.prompt = "q",
            );
            let _run_enter = run.enter();

            let tool_span = tracing::info_span!(
                "analytics.tool_call",
                oxy.name = "analytics.tool_call",
                oxy.span_type = "tool_call",
                oxy.execution_type = "semantic_query",
                oxy.is_verified = true,
            );

            async { /* simulate connector.execute_query().await */ }
                .instrument(tool_span.clone())
                .await;

            tool_span.in_scope(|| {
                tracing::info!(
                    name: "tool_call.output",
                    status = "success",
                    row_count = 3_i64,
                );
            });
            drop(tool_span);
        }

        let tool_record = rx.try_recv().expect("tool record");
        assert_eq!(tool_record.span_name, "analytics.tool_call");
        let tool_attrs: HashMap<String, String> =
            serde_json::from_str(&tool_record.span_attributes).unwrap();
        assert_eq!(
            tool_attrs.get("oxy.span_type").map(String::as_str),
            Some("tool_call")
        );
        assert_eq!(
            tool_attrs.get("oxy.execution_type").map(String::as_str),
            Some("semantic_query")
        );

        let events: Vec<serde_json::Value> = serde_json::from_str(&tool_record.event_data).unwrap();
        assert_eq!(
            events.len(),
            1,
            "tool_call.output event must be attached to tool_span"
        );
        assert_eq!(events[0]["name"], "tool_call.output");
        assert_eq!(events[0]["attributes"]["status"], "success");
    }

    /// The invariant `custom_apps_functions::runtime::run` depends on.
    ///
    /// Oxy Functions execute the V8 isolate on its own OS thread and emit every
    /// `ctx.log()` / `console.*` line from there. `tracing`'s current span is
    /// THREAD-LOCAL, so those events attach to nothing unless a span is entered
    /// *on that thread* — and [`SpanCollectorLayer::on_event`] drops an event
    /// that has no span. Before that, the entire route-mode function log stream
    /// was discarded this way.
    ///
    /// The production span is parentless rather than a clone of the caller's
    /// (see `custom_apps_functions::runtime::run`), because an isolate thread
    /// that outlives a cancel or a timeout would otherwise hold the caller's
    /// span open and inflate its duration. What this test pins is the mechanism
    /// underneath both: entering on the thread is what makes the event land.
    ///
    /// The subscriber is re-set inside each spawned thread because a test may
    /// only use `set_default`, which is thread-local. Production installs it
    /// with `.init()` (a global default every thread already sees), so carrying
    /// the span is the only half that has to be done by hand there.
    #[tokio::test]
    async fn test_event_from_isolate_thread_needs_the_span_carried_over() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let layer = SpanCollectorLayer::new(tx, "test-service".to_string());
        let subscriber = std::sync::Arc::new(tracing_subscriber::registry().with(layer));
        let _guard = tracing::subscriber::set_default(subscriber.clone());

        // (a) The bug: the worker thread does not enter the invocation span.
        {
            let span = tracing::info_span!("custom_app_function", app_id = "app-1");
            let _enter = span.enter();
            let sub = subscriber.clone();
            std::thread::spawn(move || {
                let _dispatch = tracing::subscriber::set_default(sub);
                tracing::info!(target: "custom_app_function", name = "function_log", "dropped");
            })
            .join()
            .unwrap();
        }
        let lost = rx.try_recv().expect("span record for app-1");
        let lost_events: Vec<serde_json::Value> = serde_json::from_str(&lost.event_data).unwrap();
        assert!(
            lost_events.is_empty(),
            "an event emitted from another thread WITHOUT the span carried over \
             must be dropped. If this starts capturing, entering a span on the \
             isolate thread in custom_apps_functions::runtime::run is redundant \
             and this test is guarding nothing: {lost_events:?}"
        );

        // (b) The fix: same event, span carried onto the thread.
        {
            let span = tracing::info_span!("custom_app_function", app_id = "app-2");
            let _enter = span.enter();
            let sub = subscriber.clone();
            let carried = span.clone();
            std::thread::spawn(move || {
                let _dispatch = tracing::subscriber::set_default(sub);
                let _entered = carried.enter();
                tracing::info!(
                    target: "custom_app_function",
                    name = "function_log",
                    log_level = "info",
                    "kept"
                );
            })
            .join()
            .unwrap();
        }
        let kept = rx.try_recv().expect("span record for app-2");
        let kept_events: Vec<serde_json::Value> = serde_json::from_str(&kept.event_data).unwrap();
        assert_eq!(
            kept_events.len(),
            1,
            "a span entered on the worker thread must capture its log line"
        );
        assert_eq!(kept_events[0]["name"], "function_log");
        assert_eq!(kept_events[0]["attributes"]["message"], "kept");
        assert_eq!(kept_events[0]["attributes"]["log_level"], "info");

        let attrs: HashMap<String, String> = serde_json::from_str(&kept.span_attributes).unwrap();
        assert_eq!(
            attrs.get("app_id").map(String::as_str),
            Some("app-2"),
            "the log line must arrive attributed to its app"
        );
    }

    #[tokio::test]
    async fn test_error_event_sets_status() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let layer = SpanCollectorLayer::new(tx, "test-service".to_string());

        let subscriber = tracing_subscriber::registry().with(layer);
        let _guard = tracing::subscriber::set_default(subscriber);

        {
            let span = tracing::info_span!("error_span");
            let _enter = span.enter();
            tracing::error!(message = "something went wrong");
        }

        let record = rx.try_recv().expect("should receive a SpanRecord");
        assert_eq!(record.status_code, "ERROR");
        assert_eq!(record.status_message, "something went wrong");
    }

    #[tokio::test]
    async fn test_on_record_merges_attributes() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let layer = SpanCollectorLayer::new(tx, "test-service".to_string());

        let subscriber = tracing_subscriber::registry().with(layer);
        let _guard = tracing::subscriber::set_default(subscriber);

        {
            let span = tracing::info_span!("record_span", initial = "first");
            let _enter = span.enter();
            span.record("initial", "updated");
        }

        let record = rx.try_recv().expect("should receive a SpanRecord");
        let attrs: HashMap<String, String> = serde_json::from_str(&record.span_attributes).unwrap();
        assert_eq!(attrs.get("initial").map(|s| s.as_str()), Some("updated"));
    }
}
