//! Event system for the agentic pipeline.
//!
//! Every state transition, LLM call, tool invocation, and validation result is
//! observable through an [`EventStream`].  Domain implementations can extend
//! the event vocabulary by implementing [`DomainEvents`] and wrapping their
//! events in [`Event::Domain`].
//!
//! # Quick-start
//!
//! ```rust,no_run
//! use tokio::sync::mpsc;
//! use agentic_core::events::{Event, EventStream};
//!
//! let (tx, mut rx): (EventStream, _) = mpsc::channel(256);
//! // pass `tx` to `Orchestrator::with_events`
//! ```

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

// ── HumanInputQuestion ────────────────────────────────────────────────────────

/// A single question the LLM poses to the user, with optional suggested answers.
///
/// Used in [`CoreEvent::AwaitingHumanInput`] to support multiple simultaneous
/// questions (e.g. when triage finds several independent ambiguities).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumanInputQuestion {
    /// The question text to display to the user.
    pub prompt: String,
    /// LLM-generated answer suggestions (2–4 options). May be empty.
    pub suggestions: Vec<String>,
}

// ── Outcome ───────────────────────────────────────────────────────────────────

/// The outcome of leaving a pipeline stage, carried by [`CoreEvent::StateExit`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The stage completed successfully and the pipeline moved forward.
    Advanced,
    /// The stage was retried (returned to the same stage).
    Retry,
    /// The pipeline jumped back to an earlier stage.
    #[serde(rename = "backtracked")]
    BackTracked,
    /// A fatal error terminated the run.
    Failed,
    /// The pipeline suspended to await human input; the run will resume later.
    Suspended,
}

// ── CoreEvent ─────────────────────────────────────────────────────────────────

/// Built-in orchestrator and worker events emitted on every run.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event_type", rename_all = "snake_case")]
pub enum CoreEvent {
    /// A pipeline stage was entered.
    StateEnter {
        /// Lower-cased stage name, e.g. `"clarifying"`.
        state: String,
        /// How many times this stage has been entered in the current run
        /// (0 on first entry, incremented on each re-entry via a back-edge).
        revision: u32,
        /// Opaque run identifier shared by all events in one `run()` call.
        trace_id: String,
        /// When inside a concurrent fan-out, identifies which sub-spec this
        /// event belongs to.  `None` outside fan-out.
        #[serde(skip_serializing_if = "Option::is_none")]
        sub_spec_index: Option<usize>,
    },

    /// A pipeline stage was exited.
    StateExit {
        state: String,
        outcome: Outcome,
        trace_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        sub_spec_index: Option<usize>,
    },

    /// The pipeline jumped backward to an earlier stage.
    BackEdge {
        from: String,
        to: String,
        /// Human-readable explanation.
        reason: String,
        trace_id: String,
    },

    /// The LLM provider stream is starting; emitted once per HTTP round,
    /// before the first token arrives.
    LlmStart {
        state: String,
        /// Rough estimate of prompt tokens (input / 4).
        prompt_tokens: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        sub_spec_index: Option<usize>,
    },

    /// A single output token was received from the LLM.
    LlmToken {
        token: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        sub_spec_index: Option<usize>,
    },

    /// The LLM provider stream has ended; emitted once per HTTP round,
    /// after all tokens for that round have been consumed.
    LlmEnd {
        state: String,
        output_tokens: usize,
        /// Provider-reported uncached input tokens of this round (cache reads and
        /// writes excluded). 0 when the emitter does not report it: the FSM tool
        /// loop reports prompt size on `LlmStart`; the react driver reports it here.
        #[serde(default)]
        input_tokens: usize,
        /// Tokens written to the prompt cache on this round (Anthropic only).
        /// Persisted rows from before caching support deserialise as 0.
        #[serde(default)]
        cache_creation_input_tokens: usize,
        /// Tokens read from the prompt cache on this round (Anthropic only).
        #[serde(default)]
        cache_read_input_tokens: usize,
        duration_ms: u64,
        /// The model identifier used for this completion (e.g. `"claude-sonnet-4-6"`).
        model: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        sub_spec_index: Option<usize>,
    },

    /// The model began emitting a thinking / reasoning block.
    ThinkingStart {
        state: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        sub_spec_index: Option<usize>,
    },

    /// A single token of human-readable thinking text was received.
    ThinkingToken {
        token: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        sub_spec_index: Option<usize>,
    },

    /// The model finished the current thinking block.
    ThinkingEnd {
        state: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        sub_spec_index: Option<usize>,
    },

    /// A tool invocation was dispatched.
    ToolCall {
        name: String,
        input: String,
        /// LLM inference time for the round that produced this tool call (ms).
        #[serde(default)]
        llm_duration_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        sub_spec_index: Option<usize>,
    },

    /// A tool invocation completed.
    ToolResult {
        name: String,
        output: String,
        duration_ms: u64,
        #[serde(default)]
        is_error: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        sub_spec_index: Option<usize>,
    },

    /// Stage-level validation passed.
    ValidationPass { state: String },

    /// Stage-level validation failed.
    ValidationFail { state: String, errors: Vec<String> },

    /// The specifying stage produced multiple independent specs (fan-out).
    ///
    /// Emitted once per `run()` call when `specify()` returns N > 1 specs.
    FanOut { spec_count: usize, trace_id: String },

    /// A fan-out sub-spec solve+execute is starting.
    SubSpecStart {
        index: usize,
        total: usize,
        trace_id: String,
    },

    /// A fan-out sub-spec solve+execute completed.
    SubSpecEnd { index: usize, trace_id: String },

    /// The pipeline completed successfully.
    Done { trace_id: String },

    /// The run ended with an error.
    Error { message: String, trace_id: String },

    /// The pipeline suspended because the LLM called `ask_user` and a
    /// [`DeferredInputProvider`] is wired.
    ///
    /// The caller should persist the accompanying [`SuspendedRunData`],
    /// present the `questions` to the user, and call
    /// [`Orchestrator::resume`] with the user's answer on the next turn.
    ///
    /// [`DeferredInputProvider`]: crate::human_input::DeferredInputProvider
    /// [`SuspendedRunData`]: crate::human_input::SuspendedRunData
    /// [`Orchestrator::resume`]: crate::orchestrator::Orchestrator::resume
    #[serde(rename = "awaiting_input")]
    AwaitingHumanInput {
        /// One or more questions the LLM posed (triage may produce multiple).
        questions: Vec<HumanInputQuestion>,
        /// The pipeline stage that suspended.
        from_state: String,
        /// Trace ID shared by all events in this run.
        trace_id: String,
    },

    /// The suspended run received an answer and is resuming.
    ///
    /// Pairs with the preceding [`AwaitingHumanInput`] event — the
    /// `trace_id` field matches the one on `AwaitingHumanInput` so the
    /// frontend can correlate the open/close pair.
    ///
    /// The answer may come from a human (typed response) or from a
    /// delegation (child task output).
    ///
    /// [`AwaitingHumanInput`]: CoreEvent::AwaitingHumanInput
    #[serde(rename = "input_resolved")]
    InputResolved {
        /// The answer text (human response or delegation output).
        answer: String,
        /// Trace ID matching the corresponding `awaiting_input` event.
        trace_id: String,
    },

    // ── Delegation events ────────────────────────────────────────────────
    /// A delegation to a child task has started.
    DelegationStarted {
        child_task_id: String,
        /// Human-readable target description (e.g. `"agent:builder"` or
        /// `"workflow:revenue.procedure.yml"`).
        target: String,
        /// The request/instruction sent to the child.
        request: String,
        trace_id: String,
    },

    /// A forwarded event from a running child task.
    ///
    /// The coordinator wraps child events in this envelope so the parent's
    /// SSE stream shows delegation progress.
    DelegationEvent {
        child_task_id: String,
        /// The original event type from the child (e.g. `"step_start"`).
        inner_event_type: String,
        /// The original serialized payload from the child.
        inner_payload: serde_json::Value,
    },

    /// A child delegation completed (successfully or not).
    DelegationCompleted {
        child_task_id: String,
        success: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        answer: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        trace_id: String,
    },
}

// ── DomainEvents ──────────────────────────────────────────────────────────────

/// Marker trait for domain-specific event payloads.
///
/// Implement this on your domain's event enum and use
/// `Event<YourEvent>` / `EventStream<YourEvent>` throughout.
///
/// The default implementation for `()` is provided so generic code that does
/// not need domain events can use `Event<()>` with no boilerplate.
///
/// Domain events must implement `Serialize` because they are persisted as JSON
/// via the event bridge and streamed to clients via SSE.
pub trait DomainEvents: serde::Serialize + Send + 'static {}

impl DomainEvents for () {}

// ── Event wrapper ─────────────────────────────────────────────────────────────

/// A single event on the event stream.
///
/// `D` defaults to `()` for pipelines that don't need domain-specific events.
pub enum Event<D: DomainEvents = ()> {
    /// A built-in orchestrator or worker event.
    Core(CoreEvent),
    /// A domain-specific event emitted by a worker implementation.
    Domain(D),
}

impl<D: DomainEvents> Event<D> {
    /// Serialize this event into `(event_type, payload)` for DB storage.
    ///
    /// Both `CoreEvent` and domain events must use
    /// `#[serde(tag = "event_type")]` internally-tagged format.
    pub fn serialize(&self) -> (String, serde_json::Value) {
        let value = match self {
            Event::Core(e) => {
                serde_json::to_value(e).expect("CoreEvent serialization is infallible")
            }
            Event::Domain(e) => {
                serde_json::to_value(e).expect("DomainEvent serialization is infallible")
            }
        };
        split_tagged(value)
    }
}

/// Split a serde internally-tagged enum value into `(event_type, payload)`.
///
/// Internally tagged enums serialize as `{ "event_type": "...", ...fields }`.
/// This helper removes the tag key and returns it alongside the remaining object.
fn split_tagged(v: serde_json::Value) -> (String, serde_json::Value) {
    let serde_json::Value::Object(mut obj) = v else {
        panic!("internally tagged enum always serializes to an object");
    };
    let event_type = obj
        .remove("event_type")
        .and_then(|v| {
            if let serde_json::Value::String(s) = v {
                Some(s)
            } else {
                None
            }
        })
        .unwrap_or_default();
    (event_type, serde_json::Value::Object(obj))
}

// ── EventStream type alias ─────────────────────────────────────────────────────

/// The sender half of an event channel.
///
/// Pass a clone to the [`Orchestrator`] (via [`Orchestrator::with_events`]) and
/// another clone to each worker that should emit domain events.
///
/// [`Orchestrator`]: crate::orchestrator::Orchestrator
/// [`Orchestrator::with_events`]: crate::orchestrator::Orchestrator::with_events
pub type EventStream<D = ()> = mpsc::Sender<Event<D>>;
