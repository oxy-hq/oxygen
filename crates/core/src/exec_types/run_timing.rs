//! Per-run wall-time breakdown of one eval target run (BIRD baseline): where a
//! question's time went — startup, LLM rounds, tools, warehouse queries.
//!
//! Pure data. The accumulator that fills it from the agentic event stream lives
//! in `oxy-api-eval` (`builders/run_timing.rs`), because this crate
//! must not depend on any `agentic-*` crate. All durations are milliseconds.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Failed tool calls kept in [`RunTiming::tool_errors`].
pub const MAX_TOOL_ERRORS: usize = 20;
/// Characters kept of each failed tool call's output.
pub const MAX_TOOL_ERROR_CHARS: usize = 300;

/// LLM rounds attributed to one pipeline state.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmStateTiming {
    pub calls: u64,
    pub ms: u64,
}

/// Executions of one tool, by name.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolTiming {
    pub calls: u64,
    pub ms: u64,
    pub errors: u64,
}

/// The memory reflection turn of a react eval run with memory writes on: the
/// model turn after the answer that may save notes, and the writes it made.
/// None of it is counted in the run's other fields or its `duration_ms`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReflectTiming {
    /// The reflection's start → run returns.
    pub ms: u64,
    pub llm_calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    /// Advisor calls in the turn and their tokens, at the advisor's rates and
    /// not counted in the fields above.
    #[serde(default)]
    pub advisor_calls: u64,
    #[serde(default)]
    pub advisor_input_tokens: u64,
    #[serde(default)]
    pub advisor_output_tokens: u64,
    /// `remember` / `forget` calls that ran.
    pub memory_writes: u64,
    /// `<tool>: <output>` of each failed call in the turn, each cut to
    /// [`MAX_TOOL_ERROR_CHARS`] characters: why a note was not saved.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

/// Where one target run's wall time went. When `fanout` is true the sums can
/// exceed `wall_ms` (sub-specs run concurrently) and `other_ms` is `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunTiming {
    /// Run start (just before the agent run is invoked) → run returns.
    pub wall_ms: u64,
    /// Run start → first event received (config / catalog / connection setup).
    pub startup_ms: u64,
    pub llm_calls: u64,
    /// Σ `LlmEnd.duration_ms`.
    pub llm_ms: u64,
    /// LLM rounds that streamed a token, so their time to first token was
    /// measured. Rounds with a `response_schema` (structured output suppresses
    /// `LlmToken`) and rounds that only call tools stream no token and are
    /// excluded; they still count in `llm_calls` / `llm_ms`.
    pub llm_ttft_rounds: u32,
    /// Σ over the `llm_ttft_rounds` measured rounds only: first
    /// `LlmToken`/`ThinkingToken` receipt − `LlmStart` receipt. Covers
    /// token-streaming rounds only; schema and tool-call-only rounds are excluded.
    pub llm_ttft_ms: u64,
    /// Σ over the same measured rounds only: round `duration_ms` − its TTFT,
    /// floored at 0. Covers token-streaming rounds only; schema and
    /// tool-call-only rounds are excluded, so `llm_ttft_ms + llm_stream_ms`
    /// can be well below `llm_ms`.
    pub llm_stream_ms: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    /// Advisor calls (react with an `llm.advisor`) and their tokens, billed at
    /// the advisor model's rates and not counted in the fields above. Zero in
    /// records written before the fields.
    #[serde(default)]
    pub advisor_calls: u64,
    #[serde(default)]
    pub advisor_input_tokens: u64,
    #[serde(default)]
    pub advisor_output_tokens: u64,
    pub llm_by_state: BTreeMap<String, LlmStateTiming>,
    pub tool_calls: u64,
    /// Σ `ToolResult.duration_ms`.
    pub tool_ms: u64,
    pub tools_by_name: BTreeMap<String, ToolTiming>,
    pub query_count: u64,
    /// Σ `QueryExecuted.duration_ms`.
    pub query_ms: u64,
    pub query_failures: u64,
    /// `QueryExecuted` events by `source` (`semantic`, `llm`, `vendor`,
    /// `verified_sql`): whether each execution ran SQL the semantic model
    /// compiled or SQL the model wrote. Empty in records written before the field.
    #[serde(default)]
    pub queries_by_source: BTreeMap<String, u64>,
    /// `source` of the execution whose SQL is the run's `final_sql` (the SQL a
    /// react answer cites, else the FSM's last successful query). `None` when
    /// the run has no final SQL, and in records written before the field.
    #[serde(default)]
    pub answer_source: Option<String>,
    /// `<tool>: <output>` of each failed tool call, in order: the first
    /// [`MAX_TOOL_ERRORS`], each cut to [`MAX_TOOL_ERROR_CHARS`] characters.
    /// Says why calls failed where `tools_by_name` only counts them.
    #[serde(default)]
    pub tool_errors: Vec<String>,
    /// State name → total ms between `StateEnter` and the matching `StateExit`.
    pub states: BTreeMap<String, u64>,
    /// The run fanned out: a `FanOut` event, a `SubSpecStart`/`SubSpecEnd`
    /// event, or any event tagged with a `sub_spec_index` was seen.
    pub fanout: bool,
    /// `wall − startup − llm − tool − query`, floored at 0; `None` under fan-out.
    /// Serialized as `null` (not omitted) when `None`.
    pub other_ms: Option<u64>,
    /// The memory reflection turn, kept out of every field above. `None` when
    /// the run did not reflect, and in records written before the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reflect: Option<ReflectTiming>,
}
