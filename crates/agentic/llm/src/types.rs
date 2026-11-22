use serde_json::Value;

// ── InitialMessages ───────────────────────────────────────────────────────────

/// The opening message(s) for a [`LlmClient::run_with_tools`] call.
///
/// - [`User`] — the normal path: the user's raw question, sent as a single
///   `{"role":"user","content":…}` message.
/// - [`Messages`] — pre-built message list, used on resume to inject the
///   synthetic `[user, assistant(ask_user), tool_result(answer)]` exchange
///   so the LLM sees the prior question/answer before continuing.
///
/// [`User`]: InitialMessages::User
/// [`Messages`]: InitialMessages::Messages
pub enum InitialMessages {
    /// A single user message (plain question text).
    User(String),
    /// A pre-built list of provider-native JSON message objects.
    ///
    /// Used for synthetic re-entry on resume: the caller constructs the
    /// `[user_msg, assistant(ask_user), tool_result(answer)]` sequence using
    /// [`LlmProvider::assistant_message`] and [`LlmProvider::tool_result_messages`],
    /// then passes it here.  The tool loop starts at round 0 with these messages
    /// already in the history.
    Messages(Vec<serde_json::Value>),
}

impl From<&str> for InitialMessages {
    fn from(s: &str) -> Self {
        InitialMessages::User(s.to_string())
    }
}

impl From<String> for InitialMessages {
    fn from(s: String) -> Self {
        InitialMessages::User(s)
    }
}

impl From<&String> for InitialMessages {
    fn from(s: &String) -> Self {
        InitialMessages::User(s.clone())
    }
}

// ── Thinking config ───────────────────────────────────────────────────────────

/// Reasoning effort level.
///
/// Originally OpenAI-only. Anthropic now takes the same idea as
/// `output_config.effort`, and on Claude 4.7+ it is the *only* graded control
/// over thinking depth -- `budget_tokens` was removed there, so this enum is
/// how a caller asks for less thinking on a current Claude model.
///
/// `XHigh` and `Max` exist because Anthropic accepts five levels. OpenAI's
/// `reasoning.effort` takes three, so the OpenAI provider clamps the top two
/// to `high` rather than sending a value that endpoint would reject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

impl ReasoningEffort {
    pub(super) fn as_str(&self) -> &'static str {
        match self {
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High => "high",
            ReasoningEffort::XHigh => "xhigh",
            ReasoningEffort::Max => "max",
        }
    }

    /// The nearest level OpenAI's `reasoning.effort` accepts.
    pub(super) fn as_openai_str(&self) -> &'static str {
        match self {
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High | ReasoningEffort::XHigh | ReasoningEffort::Max => "high",
        }
    }
}

/// Thinking / reasoning configuration passed to each LLM call.
///
/// The variant states what the CALLER wants. On Anthropic it is no longer a
/// compatibility choice: the provider rewrites whichever variant it is handed
/// into the shape the target model accepts, in either direction, warning once.
/// So pick the variant that says what you mean, not the one your model happens
/// to take. (The OpenAI providers pass `Effort` through and clamp its top two
/// levels; they have no equivalent translation.)
///
/// | Variant | Means | Sent natively on |
/// |---------|-------|------------------|
/// | `Disabled` | no extended thinking | any model |
/// | `Adaptive` | the model decides when and how much | Claude 4.6+ |
/// | `Manual` | a hard token ceiling | Claude 3.7 - 4.6 |
/// | `Effort` | a depth hint, five levels | Claude 4.6+ (`xhigh` 4.7+), OpenAI |
///
/// Outside a variant's native range the Anthropic provider converts: `Manual`
/// becomes `Effort` on 4.7+, `Adaptive` and `Effort` become `Manual` below 4.6,
/// and `xhigh` clamps to `high` below 4.7. Nothing below Claude 3.7 supports
/// extended thinking at all, and no rewrite helps there.
#[derive(Debug, Clone, Default)]
pub enum ThinkingConfig {
    /// No extended thinking (default).
    #[default]
    Disabled,
    /// The model decides when to think and for how long. Sends
    /// `"thinking": {"type": "adaptive"}`; below Claude 4.6, where that does
    /// not exist, the Anthropic provider sends a mid-range budget instead.
    Adaptive,
    /// A hard ceiling on thinking tokens. Sends
    /// `"thinking": {"type": "enabled", "budget_tokens": N}` -- native on
    /// Claude 3.7 through 4.6; on 4.7+ the provider converts it to an effort
    /// level, since that form is rejected there.
    Manual { budget_tokens: u32 },
    /// Graded reasoning effort.
    ///
    /// OpenAI: `"reasoning": {"effort": ...}`.
    /// Anthropic: `"thinking": {"type": "adaptive"}` plus
    /// `"output_config": {"effort": ...}` -- the only way to ask a Claude 4.7+
    /// model for less thinking, since `Manual` is rejected there.
    Effort(ReasoningEffort),
}

// ── Structured output ─────────────────────────────────────────────────────────

impl ThinkingConfig {
    /// The effort level this config carries, if any. Test/introspection helper.
    pub fn effort_level(&self) -> Option<ReasoningEffort> {
        match self {
            ThinkingConfig::Effort(e) => Some(*e),
            _ => None,
        }
    }
}

/// Describes a structured output schema for constrained LLM responses.
///
/// When set on [`ToolLoopConfig`], the provider uses its native constrained
/// decoding mechanism to guarantee the response matches this schema:
/// - **Anthropic**: native `output_config.format` (structured outputs API).
/// - **OpenAI**: `response_format: {type: "json_schema", ...}` with `strict: true`.
#[derive(Debug, Clone)]
pub struct ResponseSchema {
    /// Machine-readable name for the response schema (e.g. `"clarify_response"`).
    pub name: String,
    /// JSON Schema object describing the expected response shape.
    pub schema: Value,
}

// ── Content blocks ────────────────────────────────────────────────────────────

/// A parsed content block from an LLM response.
///
/// Thinking and redacted-thinking blocks store the **complete** provider-native
/// JSON object in `provider_data` so they can be passed back verbatim during
/// tool-use loops.  Callers must not inspect or mutate `provider_data`.
///
/// # Lifetime constraint
///
/// Thinking blocks MUST NOT cross FSM state boundaries.  The orchestrator
/// discards [`LlmOutput::raw_content_blocks`] when transitioning between
/// states.  Only the human-readable thinking summary (carried by
/// [`CoreEvent::ThinkingToken`] events) may be retained for logging / display.
#[derive(Debug, Clone)]
pub enum ContentBlock {
    /// Extended thinking block.  Contains the provider's encrypted blob
    /// (Anthropic: `signature`; OpenAI: `encrypted_content`).
    /// Pass back verbatim in the next assistant turn.
    Thinking { provider_data: Value },
    /// Redacted thinking block (Claude-specific).  Opaque; pass back verbatim.
    RedactedThinking { provider_data: Value },
    /// Ordinary text output.
    Text { text: String },
    /// A tool invocation requested by the model.
    ToolUse {
        id: String,
        name: String,
        input: Value,
        /// Complete provider-native JSON for this tool call.  When present,
        /// [`LlmProvider::assistant_message`] should pass it back verbatim
        /// instead of reconstructing from the parsed fields.  This is
        /// required by the OpenAI Responses API, which expects the full
        /// output item (including `id`, `status`, etc.) in the next input.
        provider_data: Option<Value>,
    },
    /// A block of a tool the provider runs itself (Anthropic's advisor:
    /// `server_tool_use` and `advisor_tool_result`). Opaque: pass back
    /// verbatim, in stream order. Other providers never produce one.
    ServerTool { provider_data: Value },
}

// ── Streaming chunk types ─────────────────────────────────────────────────────

/// A complete tool invocation carried in a [`Chunk::ToolCall`].
///
/// Emitted once per tool call after the full input JSON has been accumulated
/// from the stream.
#[derive(Debug, Clone)]
pub struct ToolCallChunk {
    pub id: String,
    pub name: String,
    pub input: Value,
    /// Complete provider-native JSON for this tool call, stored for verbatim
    /// passback during tool-use loops.  `None` for providers that reconstruct
    /// the wire format from the parsed fields (e.g. Anthropic).
    pub provider_data: Option<Value>,
}

/// A single item in the streaming output from an [`LlmProvider`].
#[derive(Debug)]
pub enum Chunk {
    /// Human-readable thinking text (Anthropic thinking block / OpenAI
    /// reasoning summary).  May arrive in multiple chunks.  Encrypted blobs
    /// are emitted separately as [`Chunk::RawBlock`].
    ThinkingSummary(String),
    /// A content text token.
    Text(String),
    /// A complete tool call, emitted after the full input JSON is received.
    ToolCall(ToolCallChunk),
    /// An opaque provider blob (encrypted thinking / signature) that must be
    /// passed back verbatim in tool continuation turns.
    RawBlock(ContentBlock),
    /// The stream finished; carries total token usage.
    Done(Usage),
}

// ── Output types ──────────────────────────────────────────────────────────────

/// Why the model stopped generating.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum StopReason {
    /// Normal completion — the model emitted a stop token.
    #[default]
    EndTurn,
    /// The response was truncated because `max_tokens` was reached.
    MaxTokens,
    /// The model elected to use a tool.
    ToolUse,
    /// A provider-run tool (the advisor) is still pending: resend with this
    /// response appended as the assistant message, adding no user message.
    PauseTurn,
}

/// Token usage reported by the provider for a single completion.
#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub input_tokens: usize,
    pub output_tokens: usize,
    /// Tokens written to the prompt cache on this call (Anthropic only).
    /// Charged at 1.25× base for 5m TTL.  Zero on providers without
    /// caching support.
    pub cache_creation_input_tokens: usize,
    /// Tokens read from the prompt cache on this call (Anthropic only).
    /// Charged at 0.1× base.  Zero on providers without caching support.
    pub cache_read_input_tokens: usize,
    /// Why the model stopped; populated by provider stream parsers.
    pub stop_reason: StopReason,
    /// Advisor calls inside this completion, billed at the advisor model's
    /// rates and NOT included in the token fields above.
    pub advisor: AdvisorUsage,
}

/// Tokens of the advisor calls in one completion (Anthropic's
/// `usage.iterations` entries of type `advisor_message`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AdvisorUsage {
    pub calls: u32,
    /// The advisor model, as the provider reported it; empty with no calls.
    pub model: String,
    pub input_tokens: usize,
    pub output_tokens: usize,
    pub cache_creation_input_tokens: usize,
    pub cache_read_input_tokens: usize,
}

impl AdvisorUsage {
    /// Adds `other`'s calls and tokens; keeps the first model seen.
    pub fn add(&mut self, other: &AdvisorUsage) {
        self.calls += other.calls;
        if self.model.is_empty() {
            self.model.clone_from(&other.model);
        }
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_creation_input_tokens += other.cache_creation_input_tokens;
        self.cache_read_input_tokens += other.cache_read_input_tokens;
    }
}

/// The final output produced by a completion or tool-use loop.
#[derive(Debug, Clone)]
pub struct LlmOutput {
    /// The assistant's final text response.
    pub text: String,
    /// Human-readable summary of thinking blocks seen in the final response
    /// round, for display / event logging only.  Never re-injected into the LLM.
    pub thinking_summary: Option<String>,
    /// Raw content blocks from the **final** response round.  The orchestrator
    /// MUST discard these when transitioning between FSM states.
    pub raw_content_blocks: Vec<ContentBlock>,
    /// Structured JSON response extracted from a forced tool call (Anthropic)
    /// or constrained decoding (OpenAI). `None` when no `response_schema` was set.
    pub structured_response: Option<Value>,
    /// Every tool call made during the loop, in order: `(tool_name, input_params)`.
    ///
    /// Lets callers inspect tool-use side-effects after the loop without
    /// resorting to `Arc<Mutex>` captures inside the tool executor closure.
    pub tool_calls: Vec<(String, Value)>,
    /// Full provider-native message history at loop completion (user + all
    /// tool_use/tool_result rounds + final assistant turn). Can be passed
    /// directly to the next LLM call as `InitialMessages::Messages` to give
    /// it full, unabridged context of everything that happened.
    pub prior_messages: Vec<Value>,
}

// ── Tool loop configuration ───────────────────────────────────────────────────

/// Configuration for [`LlmClient::run_with_tools`].
#[derive(Debug, Clone)]
pub struct ToolLoopConfig {
    /// Maximum number of tool-call rounds before returning
    /// [`LlmError::MaxToolRoundsExceeded`].
    pub max_tool_rounds: u32,
    /// Pipeline state name, used for [`CoreEvent::LlmStart`] /
    /// [`CoreEvent::LlmEnd`] / thinking event fields.
    pub state: String,
    /// Thinking / reasoning mode for this state's LLM calls.
    pub thinking: ThinkingConfig,
    /// Optional JSON Schema for structured output.
    /// When set, the provider uses constrained decoding to guarantee
    /// the response matches this schema exactly.
    pub response_schema: Option<ResponseSchema>,
    /// Optional explicit `max_tokens` override.  When `Some`, the provider
    /// uses this value instead of computing one from `ThinkingConfig`.
    /// Used by the truncation-retry logic in [`LlmClient::run_with_tools`].
    pub max_tokens_override: Option<u32>,
    /// When inside a concurrent fan-out, identifies which sub-spec this
    /// tool loop belongs to.  Propagated to all emitted [`CoreEvent`]s.
    pub sub_spec_index: Option<usize>,
    /// Optional dynamic system-prompt suffix that is excluded from the
    /// prompt-cache prefix on Anthropic.  Used for time-varying content
    /// like "Today is YYYY-MM-DD." that would otherwise invalidate every
    /// cache hit.  Empty / `None` means no suffix is appended.
    pub system_date_hint: Option<String>,
}

impl Default for ToolLoopConfig {
    fn default() -> Self {
        Self {
            max_tool_rounds: 5,
            state: String::new(),
            thinking: ThinkingConfig::Disabled,
            response_schema: None,
            max_tokens_override: None,
            sub_spec_index: None,
            system_date_hint: None,
        }
    }
}
