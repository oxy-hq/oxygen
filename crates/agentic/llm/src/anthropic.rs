use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};

use async_stream::stream;
use async_trait::async_trait;
use futures_core::Stream;
use serde_json::{Value, json};

use agentic_core::tools::ToolDef;

use super::advisor::{ADVISOR_BETA, AdvisorConfig, ServerBlock, merge_delta_usage};
use super::constants::*;
use super::sse::{ApiError, pop_sse_event, sse_data};
use super::strict_schema::sanitize_for_strict;
use super::{
    AdvisorUsage, Chunk, ContentBlock, LlmError, LlmProvider, ReasoningEffort, ResponseSchema,
    StopReason, ThinkingConfig, ToolCallChunk, Usage,
};

// ── Model capability probes ──────────────────────────────────────────────────

/// Leading integer of `tok`, ignoring any trailing junk.
///
/// Model ids pick up suffixes -- a date (`claude-haiku-4-5-20251001`), a
/// context marker (`claude-opus-5[1m]`), a platform version (`...-v1:0`) --
/// and a strict parse would reject the whole token and lose the version.
fn leading_u32(tok: &str) -> Option<u32> {
    let digits: String = tok.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// `(major, minor)` parsed out of a Claude model id, or `None` when the id is
/// not a recognisable Claude version.
///
/// Parsed rather than table-matched so a model released after this code was
/// written classifies correctly instead of silently taking a legacy path. Both
/// id shapes work: family-first (`claude-opus-4-7`, `claude-sonnet-5`) and the
/// older number-first (`claude-3-5-sonnet-20241022`), because the first two
/// integers are the version in both. A vendor prefix
/// (`us.anthropic.claude-...`) is stripped by anchoring on the last `claude-`.
///
/// The minor is read only from the token IMMEDIATELY after the major, and only
/// when its leading digit run is short enough to be a minor. Two bugs live in
/// the alternatives, and both shipped here before this shape did:
///
/// - Scanning later tokens for "the next number" picks up the release date on
///   an id whose major has no minor segment -- `claude-sonnet-4-20250514`
///   parses as 4.20250514, i.e. "4.7 or later".
/// - Filtering on whole-token length drops a minor carrying a suffix --
///   `claude-opus-4-7[1m]` parses as 4.0.
///
/// Measuring the digit run is the same quantity [`leading_u32`] parses, so the
/// filter and the parse cannot disagree.
fn claude_version(model: &str) -> Option<(u32, u32)> {
    let lower = model.to_ascii_lowercase();
    let idx = lower.rfind("claude-")?;
    let rest = &lower[idx + "claude-".len()..];

    let mut toks = rest.split(['-', '.']).filter(|t| !t.is_empty());
    let major = toks.find_map(leading_u32)?;
    let minor = toks
        .next()
        .filter(|t| t.chars().take_while(char::is_ascii_digit).count() <= 2)
        .and_then(leading_u32)
        .unwrap_or(0);
    Some((major, minor))
}

/// Whether `model` is at least version `major.minor`.
///
/// `None` -- an unrecognised id: a proxy alias, a gateway, a local model, a
/// fine-tune -- answers **true** at every call site below, because every one of
/// them decides whether to rewrite the caller's config. Assuming an unknown id
/// is current leaves that config alone; assuming it is ancient would silently
/// downgrade a request aimed at a model this code has never heard of.
fn claude_at_least(model: &str, major_min: u32, minor_min: u32) -> bool {
    let Some((major, minor)) = claude_version(model) else {
        return true;
    };
    major > major_min || (major == major_min && minor >= minor_min)
}

/// Whether `model` still accepts `thinking: {"type": "enabled", budget_tokens}`.
///
/// Anthropic removed that form on Claude 4.7 and everything after it; those
/// models return a 400 pointing at `thinking.type.adaptive` and
/// `output_config.effort`. 4.6 and earlier still take it.
///
/// An unrecognised id keeps the legacy shape rather than being translated --
/// translating would be a guess, and this is the form that has always been sent.
fn model_accepts_budget_tokens(model: &str) -> bool {
    // `is_some_and` carries the unknown-id default in the expression itself:
    // no version parsed => false => accepted, same as every probe here. Written
    // as `match { None => true, .. }` this read as a redundant arm that was in
    // fact load-bearing, with its rationale in another function's docstring.
    !claude_version(model).is_some_and(|(major, minor)| major > 4 || (major == 4 && minor >= 7))
}

/// Whether `model` accepts the `xhigh` effort level.
///
/// `xhigh` arrived with Claude 4.7, between `high` and `max`. It is NOT the
/// same boundary as `max`, which shipped on 4.6 -- see
/// [`model_supports_adaptive_thinking`], where that boundary now lives.
fn model_accepts_xhigh_effort(model: &str) -> bool {
    claude_at_least(model, 4, 7)
}

/// Whether `model` supports `thinking: {"type": "adaptive"}`.
///
/// Adaptive arrived on 4.6. From 3.7 up to it, thinking is requested with
/// `{"type": "enabled", budget_tokens}` -- so an `Effort` config, which pairs
/// adaptive with `output_config.effort`, cannot be sent to those models at all:
/// the effort level would be clamped exactly and the request would still 400 on
/// the field beside it.
///
/// Extended thinking itself arrived on 3.7, so below THAT neither shape works
/// and no rewrite here helps. That is not a regression -- adaptive was equally
/// rejected -- and it does not earn a fourth version probe; it is only worth
/// not claiming the budgeted form is universally accepted below 4.6.
///
/// This is also where `max` becomes available (4.6 takes
/// `low`/`medium`/`high`/`max`; `xhigh` waits for 4.7). Those two boundaries
/// coinciding is why `max` needs no clamp of its own -- anything below 4.6 has
/// already been converted away from `Effort` by the time a level is written.
fn model_supports_adaptive_thinking(model: &str) -> bool {
    claude_at_least(model, 4, 6)
}

/// Nearest `budget_tokens` for an effort level that cannot be sent as one.
///
/// The inverse of [`effort_for_budget`], and coarse for the same reason: the two
/// are different units. 1024 is the API floor for a thinking budget, so every
/// bucket clears it.
fn budget_for_effort(effort: ReasoningEffort) -> u32 {
    match effort {
        ReasoningEffort::Low => 2_048,
        ReasoningEffort::Medium => 8_192,
        ReasoningEffort::High | ReasoningEffort::XHigh | ReasoningEffort::Max => 16_384,
    }
}

/// Nearest effort level for a `budget_tokens` value that can no longer be sent.
///
/// Coarse on purpose. The two are not the same unit -- a budget is a hard
/// ceiling, effort is a depth hint -- so the only honest mapping is a bucket
/// that preserves the caller's intent ("a little" / "a lot") rather than a
/// formula implying a precision that is not there. The `warn!` at the call
/// site tells the operator to set `effort` directly.
fn effort_for_budget(budget_tokens: u32) -> ReasoningEffort {
    match budget_tokens {
        0..=4_095 => ReasoningEffort::Low,
        4_096..=16_383 => ReasoningEffort::Medium,
        _ => ReasoningEffort::High,
    }
}

/// Raises `total` to `usage[key]` when that is present and larger.
fn raise_to(usage: &Value, key: &str, total: &mut usize) {
    if let Some(v) = usage[key].as_u64() {
        *total = (*total).max(v as usize);
    }
}

/// The wire `stop_reason`; anything unrecognised ends the turn.
fn parse_stop_reason(sr: &str) -> StopReason {
    match sr {
        "max_tokens" => StopReason::MaxTokens,
        "tool_use" => StopReason::ToolUse,
        "pause_turn" => StopReason::PauseTurn,
        _ => StopReason::EndTurn,
    }
}

// ── AnthropicProvider ─────────────────────────────────────────────────────────

/// Anthropic Messages API provider (streaming).
///
/// Supports every [`ThinkingConfig`] variant on every model, by rewriting
/// whichever it is handed into a shape the target accepts -- see
/// [`Self::resolve_thinking`] for the four conversions and their version
/// boundaries. A caller therefore states intent and does not have to match the
/// variant to the model.
/// Encrypted thinking blobs (type + thinking + signature) are emitted as
/// [`Chunk::RawBlock`] and passed back verbatim between tool rounds.
pub struct AnthropicProvider {
    api_key: String,
    model: String,
    /// Root of the Anthropic API, e.g. `"https://api.anthropic.com/v1"`.
    /// The `/messages` path is appended by [`Self::messages_url`], matching
    /// `OpenAiProvider` (`/responses`) and `OpenAiCompatProvider`
    /// (`/chat/completions`). All three therefore accept a model's `api_url`
    /// verbatim, which is a root in every vendor's config.
    base_url: String,
    /// `gen_ai.provider.name`, resolved once from the endpoint.
    provider_name: &'static str,
    /// Extra headers sent with every request, on top of the standard
    /// `x-api-key` / `anthropic-version` / `content-type` set.
    headers: HashMap<String, String>,
    client: reqwest::Client,
    /// Latches the first "your thinking config was rewritten" warning, so a
    /// tool loop says it once rather than once per HTTP round.
    ///
    /// ONE latch covers all three rewrite arms in [`Self::resolve_thinking`],
    /// because they partition on the MODEL and the model is fixed for the life
    /// of a provider: >=4.7 translates `Manual`, <4.6 converts `Adaptive` and
    /// `Effort` together, 4.6 clamps `xhigh`.
    ///
    /// Partitioning by model is the load-bearing part. Splitting the <4.6 case
    /// into an `Adaptive` arm and an `Effort` arm would partition those two by
    /// the caller's VARIANT instead -- which varies per request, since every
    /// state without a `model:` override shares one provider -- and a run mixing
    /// `adaptive` and `effort:` states would then silence one message. Hence one
    /// arm there, and hence this note: the tempting split is the bug.
    ///
    /// The `xhigh` clamp's own guard is "below 4.7", which also matches
    /// everything the conversion arm handles; only arm ORDER leaves it
    /// 4.6-only. Said plainly because this comment is where a reader checks the
    /// claim, and crediting the guard alone would send them to the wrong line.
    thinking_warned: AtomicBool,
    /// The advisor declared on every request, when set (react only: the FSM
    /// tool loop does not resume a `pause_turn`).
    advisor: Option<AdvisorConfig>,
}

impl AnthropicProvider {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: model.into(),
            provider_name: "anthropic",
            base_url: ANTHROPIC_BASE_URL.to_string(),
            headers: HashMap::new(),
            client: build_llm_http_client(),
            thinking_warned: AtomicBool::new(false),
            advisor: None,
        }
    }

    /// Create a provider pointed at a custom endpoint.
    ///
    /// `base_url` is the **root** of the API, e.g.
    /// `"https://api.anthropic.com/v1"` — `/messages` is appended. This is the
    /// same shape a model's `api_url` carries in `config.yml`, so a value can
    /// be passed straight through without normalisation.
    pub fn with_base_url(
        api_key: impl Into<String>,
        model: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Self {
        let mut base = base_url.into();
        while base.ends_with('/') {
            base.pop();
        }
        Self {
            api_key: api_key.into(),
            model: model.into(),
            provider_name: crate::genai::provider_name_for_url(&base, "anthropic"),
            base_url: base,
            headers: HashMap::new(),
            client: build_llm_http_client(),
            thinking_warned: AtomicBool::new(false),
            advisor: None,
        }
    }

    fn messages_url(&self) -> String {
        format!("{}/messages", self.base_url)
    }

    /// Attach extra request headers, already resolved to literal values.
    pub fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = headers;
        self
    }

    /// Declare `advisor` on every request this provider sends.
    pub fn with_advisor(mut self, advisor: AdvisorConfig) -> Self {
        self.advisor = Some(advisor);
        self
    }

    /// Adds the `anthropic-beta` header when the request needs one: the
    /// extended-thinking beta whenever any thinking mode is requested
    /// (`Effort` included), and the advisor beta whenever it is declared.
    fn with_beta_header(
        &self,
        req: reqwest::RequestBuilder,
        thinking: &ThinkingConfig,
    ) -> reqwest::RequestBuilder {
        match self.beta_header(thinking) {
            Some(betas) => req.header("anthropic-beta", betas),
            None => req,
        }
    }

    /// The `anthropic-beta` header value for a request, if it needs one.
    fn beta_header(&self, thinking: &ThinkingConfig) -> Option<String> {
        let mut betas = Vec::new();
        if !matches!(thinking, ThinkingConfig::Disabled) {
            betas.push(ANTHROPIC_THINKING_BETA);
        }
        if self.advisor.is_some() {
            betas.push(ADVISOR_BETA);
        }
        (!betas.is_empty()).then(|| betas.join(","))
    }

    fn block_to_wire(block: &ContentBlock) -> Value {
        match block {
            ContentBlock::ServerTool { provider_data } => provider_data.clone(),
            ContentBlock::Thinking { provider_data } => provider_data.clone(),
            ContentBlock::RedactedThinking { provider_data } => provider_data.clone(),
            ContentBlock::Text { text } => json!({"type": "text", "text": text}),
            ContentBlock::ToolUse {
                id, name, input, ..
            } => json!({
                "type": "tool_use",
                "id": id,
                "name": name,
                "input": input
            }),
        }
    }
}

impl AnthropicProvider {
    /// Build the JSON request body for `/v1/messages`.
    ///
    /// Pure helper — no HTTP, no I/O — so unit tests can inspect the wire
    /// format directly.  Marks the system block and the last tool with
    /// `cache_control: ephemeral` so Anthropic caches the prefix.  When
    /// `system_date_suffix` is non-empty, it is appended as a second,
    /// uncached system content block so the time-varying date string does
    /// not invalidate the cached prefix.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_request_body(
        &self,
        system: &str,
        system_date_suffix: &str,
        messages: &[Value],
        tools: &[ToolDef],
        thinking: &ThinkingConfig,
        response_schema: Option<&ResponseSchema>,
        max_tokens_override: Option<u32>,
    ) -> Value {
        let mut tools_json: Vec<Value> = tools
            .iter()
            .map(|t| {
                // Strict mode accepts only a JSON Schema subset and 400s the
                // whole request otherwise; see `strict_schema`.
                let input_schema = if t.strict {
                    sanitize_for_strict(&t.parameters)
                } else {
                    t.parameters.clone()
                };
                json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": input_schema,
                    "strict": t.strict
                })
            })
            .collect();
        // The advisor goes first so the last tool, which carries the cache
        // breakpoint below, stays a client tool.
        if let Some(advisor) = &self.advisor {
            tools_json.insert(0, advisor.tool_json());
        }

        // Thinking. Two shapes exist and which one a model accepts is not a
        // preference:
        //
        //   {"type": "adaptive"}                      4.6+   the current form
        //   {"type": "enabled", "budget_tokens": N}   <=4.6  removed on 4.7+
        //
        // On a 4.7+ model the second returns 400 "thinking.type.enabled is not
        // supported for this model", so a `Manual` config that parses and
        // validates still kills the request at run time. Translate it rather
        // than forwarding a shape the model will refuse.
        //
        // `Effort` used to collapse to bare `Adaptive`, which threw the level
        // away and left callers with no way to ask for less thinking at all.
        // It is now what the API's own error message points at: adaptive
        // thinking plus `output_config.effort`.
        //
        // Resolved HERE, above max_tokens, because max_tokens is sized from
        // the thinking config -- and it has to be the one actually sent. Sizing
        // from the caller's original while sending a translated one reserves
        // headroom for a `budget_tokens` the body no longer carries, so a large
        // configured budget pushes max_tokens past the model's output cap and
        // 400s for a different reason: one runtime failure swapped for another.
        let effective_thinking = self.resolve_thinking(thinking);

        // Choose max_tokens based on the thinking config being sent: Manual
        // mode requires max_tokens >= budget_tokens; Adaptive benefits from a
        // larger cap so the model can allocate freely.
        let max_tokens = max_tokens_override.unwrap_or_else(|| match &effective_thinking {
            ThinkingConfig::Manual { budget_tokens } => std::cmp::max(
                THINKING_MAX_TOKENS,
                budget_tokens.saturating_add(DEFAULT_MAX_TOKENS),
            ),
            // Effort is adaptive thinking with a depth hint, so it needs the
            // same headroom -- the model still allocates thinking tokens out
            // of max_tokens, and DEFAULT_MAX_TOKENS would cap it far too low.
            ThinkingConfig::Adaptive | ThinkingConfig::Effort(_) => THINKING_MAX_TOKENS,
            ThinkingConfig::Disabled => DEFAULT_MAX_TOKENS,
        });

        // Mark the last message's last non-thinking content block with
        // cache_control so Round N reads the prior conversation history
        // (tool calls + tool results from Rounds 1..N-1) from cache instead
        // of re-paying for it.  Uses Anthropic's third breakpoint slot.
        let mut messages_owned: Vec<Value> = messages.to_vec();
        Self::mark_last_message_for_caching(&mut messages_owned);

        let mut body = json!({
            "model": self.model,
            "max_tokens": max_tokens,
            "system": Self::build_system_blocks(system, system_date_suffix),
            "messages": messages_owned,
            "stream": true,
        });

        // Structured output: when no real tools, use output_config (constrained
        // decoding).  When real tools are present, inject a synthetic tool so the
        // model can call it when done — output_config + tools is unreliable on
        // smaller models (e.g. Haiku) which may return empty text.
        if let Some(schema) = response_schema {
            // Both paths decode under the strict subset.
            let strict_schema = sanitize_for_strict(&schema.schema);
            if tools.is_empty() {
                body["output_config"] = json!({
                    "format": {
                        "type": "json_schema",
                        "schema": strict_schema
                    }
                });
            } else {
                tools_json.push(json!({
                    "name": schema.name,
                    "description": "You MUST call this tool to return your final structured response. Do NOT embed JSON in your text — always use this tool.",
                    "input_schema": strict_schema,
                    "strict": true
                }));
            }
        }

        if !tools_json.is_empty() {
            // Mark the last tool with cache_control so the system + tools
            // prefix is cached.  Synthetic structured-response tools are
            // appended deterministically per state, so the array stays
            // byte-stable across rounds within one run_with_tools call.
            if let Some(last) = tools_json.last_mut() {
                last["cache_control"] = json!({"type": "ephemeral"});
            }
            body["tools"] = json!(tools_json);
        }

        match &effective_thinking {
            ThinkingConfig::Adaptive => {
                body["thinking"] = json!({"type": "adaptive"});
            }
            ThinkingConfig::Manual { budget_tokens } => {
                body["thinking"] = json!({"type": "enabled", "budget_tokens": budget_tokens});
            }
            ThinkingConfig::Effort(effort) => {
                body["thinking"] = json!({"type": "adaptive"});
                // Merge, never assign: `output_config` may already carry the
                // structured-output `format` set above, and clobbering it
                // would silently drop constrained decoding.
                body["output_config"]["effort"] = json!(effort.as_str());
            }
            ThinkingConfig::Disabled => {}
        }

        body
    }

    /// Warn once that an effort level was clamped for this model.
    ///
    /// Shares `thinking_warned` with the other rewrite warnings; see that
    /// field for why one latch cannot hide a message here.
    fn warn_effort_clamped(&self, level: ReasoningEffort) {
        if !self.thinking_warned.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                model = %self.model,
                effort = level.as_str(),
                "this effort level postdates the model and was clamped to `high`"
            );
        }
    }

    /// Resolve the caller's [`ThinkingConfig`] to the one this model accepts.
    ///
    /// Four adjustments, each "send a shape the model knows" rather than a
    /// preference:
    ///
    /// - `Manual` on a model that removed `budget_tokens` (4.7+) becomes
    ///   `Effort`.
    /// - `Adaptive` on a model that predates it (below 4.6) becomes `Manual` at
    ///   a mid-range budget. Without this, `effort` worked on a 4.5 model and a
    ///   bare `adaptive` did not, which is backwards -- `adaptive` is the more
    ///   ordinary thing to write.
    /// - `Effort` on a model without adaptive thinking (below 4.6) becomes
    ///   `Manual`. This is the only one that changes the `thinking` field's
    ///   type, and the reason clamping the level alone was not enough: the
    ///   level would be exact and the field beside it still rejected.
    /// - `Effort(XHigh)` on 4.6 clamps to `High`; `xhigh` waited for 4.7.
    ///
    /// The warning fires ONCE per provider, not once per request.
    /// `build_request_body` runs on every tool-loop round, so warning inline
    /// repeated the same line a dozen times for a single run and buried it.
    fn resolve_thinking(&self, thinking: &ThinkingConfig) -> ThinkingConfig {
        match thinking {
            ThinkingConfig::Manual { budget_tokens }
                if !model_accepts_budget_tokens(&self.model) =>
            {
                let effort = effort_for_budget(*budget_tokens);
                if !self.thinking_warned.swap(true, Ordering::Relaxed) {
                    tracing::warn!(
                        model = %self.model,
                        budget_tokens,
                        effort = effort.as_str(),
                        "thinking.budget_tokens is not supported on this model and was \
                         translated to adaptive thinking at this effort; set `effort` \
                         directly to control it"
                    );
                }
                ThinkingConfig::Effort(effort)
            }
            // Below 4.6 there is no adaptive thinking, so neither `Adaptive` nor
            // `Effort` (which is adaptive plus a level) can be sent. Both become
            // the shape those models do take.
            //
            // ONE arm, not two. They share a guard and differ only by the
            // caller's variant, which varies per REQUEST -- states without a
            // `model:` override share a provider, so a run with one state on
            // `adaptive` and another on `effort` hits both. As two arms behind
            // one latch, whichever fired first silenced the other; as one arm
            // there is one message and the latch invariant holds by model again.
            //
            // Left alone, `effort` on a 4.5 model worked while `adaptive` on the
            // same model 400'd -- and `adaptive` is what this repo's own agentic
            // template and config example pair with `claude-haiku-4-5`.
            ThinkingConfig::Adaptive | ThinkingConfig::Effort(_)
                if !model_supports_adaptive_thinking(&self.model) =>
            {
                // An `Effort` level carries the caller's intent; a bare
                // `adaptive` -- "you decide" -- has none to carry, so it lands
                // mid-range rather than at either extreme.
                let budget = match thinking {
                    ThinkingConfig::Effort(level) => budget_for_effort(*level),
                    _ => budget_for_effort(ReasoningEffort::Medium),
                };
                if !self.thinking_warned.swap(true, Ordering::Relaxed) {
                    tracing::warn!(
                        model = %self.model,
                        budget_tokens = budget,
                        "this model predates adaptive thinking, so the thinking config was \
                         sent as an explicit budget instead"
                    );
                }
                ThinkingConfig::Manual {
                    budget_tokens: budget,
                }
            }
            // Reached only for 4.6: the guard is "below 4.7", but the arm above
            // has already claimed everything below 4.6. Do not reorder these.
            // 4.6 has adaptive and `max` but not `xhigh`, which waited for 4.7.
            ThinkingConfig::Effort(level @ ReasoningEffort::XHigh)
                if !model_accepts_xhigh_effort(&self.model) =>
            {
                self.warn_effort_clamped(*level);
                ThinkingConfig::Effort(ReasoningEffort::High)
            }
            other => other.clone(),
        }
    }

    /// Mark the last non-thinking content block of the last message with
    /// `cache_control: ephemeral`.  No-op if `messages` is empty or the last
    /// message has no cacheable content.
    ///
    /// `cache_control` on a `thinking` or `redacted_thinking` block is
    /// rejected by the API, so the helper walks the last message's blocks
    /// from the end and marks the first non-thinking entry it finds.  In
    /// practice the last message is always a user/tool_result message
    /// (assistant tool_use turns are followed by their tool_result reply),
    /// so thinking blocks only appear earlier in the conversation — but the
    /// guard is cheap insurance against malformed history.
    ///
    /// String-valued `content` is lifted to a one-block array so the marker
    /// can be attached.
    fn mark_last_message_for_caching(messages: &mut [Value]) {
        let Some(last) = messages.last_mut() else {
            return;
        };
        match last.get_mut("content") {
            Some(Value::Array(blocks)) => {
                for block in blocks.iter_mut().rev() {
                    let ty = block.get("type").and_then(Value::as_str).unwrap_or("");
                    if ty == "thinking" || ty == "redacted_thinking" {
                        continue;
                    }
                    block["cache_control"] = json!({"type": "ephemeral"});
                    return;
                }
            }
            Some(Value::String(s)) => {
                let text = std::mem::take(s);
                last["content"] = json!([{
                    "type": "text",
                    "text": text,
                    "cache_control": {"type": "ephemeral"}
                }]);
            }
            _ => {}
        }
    }

    /// Construct the `system` field as a content-blocks array.
    ///
    /// - When `system` is non-empty, the static prefix gets a
    ///   `cache_control: ephemeral` breakpoint so Anthropic caches it.
    /// - When `system_date_suffix` is non-empty, it is emitted as a second
    ///   block *without* `cache_control` so daily date changes don't
    ///   invalidate the cached static prefix.
    fn build_system_blocks(system: &str, system_date_suffix: &str) -> Value {
        let mut blocks: Vec<Value> = Vec::new();
        if !system.is_empty() {
            blocks.push(json!({
                "type": "text",
                "text": system,
                "cache_control": {"type": "ephemeral"}
            }));
        }
        if !system_date_suffix.is_empty() {
            blocks.push(json!({
                "type": "text",
                "text": system_date_suffix
            }));
        }
        Value::Array(blocks)
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn stream(
        &self,
        system: &str,
        system_date_suffix: &str,
        messages: &[Value],
        tools: &[ToolDef],
        thinking: &ThinkingConfig,
        response_schema: Option<&ResponseSchema>,
        max_tokens_override: Option<u32>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Chunk, LlmError>> + Send>>, LlmError> {
        let body = self.build_request_body(
            system,
            system_date_suffix,
            messages,
            tools,
            thinking,
            response_schema,
            max_tokens_override,
        );

        let mut req = self
            .client
            .post(self.messages_url())
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json");

        req = self.with_beta_header(req, thinking);
        for (name, value) in &self.headers {
            req = req.header(name, value);
        }

        let response = req.json(&body).send().await.map_err(send_error_to_llm)?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            let text = response.text().await.unwrap_or_default();
            return Err(LlmError::Auth(text));
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            // Capture the provider's `Retry-After` hint before consuming the
            // body — the solver prefers it over blind exponential backoff.
            let retry_after = parse_retry_after(response.headers());
            let text = response.text().await.unwrap_or_default();
            return Err(LlmError::RateLimit {
                message: text,
                retry_after,
            });
        }
        // 5xx are server-side and usually transient — retry on the backoff path.
        if status.is_server_error() {
            let text = response.text().await.unwrap_or_default();
            return Err(LlmError::Transient(format!("HTTP {status}: {text}")));
        }
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            if let Ok(api_err) = serde_json::from_str::<ApiError>(&text) {
                return Err(LlmError::Http(api_err.error.message));
            }
            return Err(LlmError::Http(format!("HTTP {status}: {text}")));
        }

        let s = stream! {
            use tokio_stream::StreamExt as _;

            let mut sse_buf = String::new();
            // Current open content block
            let mut block_type: Option<String> = None;
            let mut thinking_text = String::new();
            let mut thinking_sig = String::new();
            // Tool-use accumulator
            let mut tool_id = String::new();
            let mut tool_name = String::new();
            let mut tool_args = String::new();
            let mut server = ServerBlock::default();
            let mut advisor = AdvisorUsage::default();
            // Usage
            let mut input_tokens: usize = 0;
            let mut output_tokens: usize = 0;
            let mut cache_creation_input_tokens: usize = 0;
            let mut cache_read_input_tokens: usize = 0;
            let mut stop_reason = StopReason::EndTurn;

            let mut byte_stream = response.bytes_stream();

            'outer: while let Some(bytes_result) = byte_stream.next().await {
                let bytes = match bytes_result {
                    Ok(b) => b,
                    Err(e) => {
                        // A network error mid-stream is transient — surface it
                        // as such so the solver can retry on the backoff path.
                        yield Err(LlmError::Transient(e.to_string()));
                        return;
                    }
                };
                sse_buf.push_str(&String::from_utf8_lossy(&bytes));

                loop {
                    let event_text = match pop_sse_event(&mut sse_buf) {
                        Some(e) => e,
                        None => break,
                    };

                    let data = match sse_data(&event_text) {
                        Some(d) if !d.is_empty() => d,
                        _ => continue,
                    };

                    if data == "[DONE]" {
                        break 'outer;
                    }

                    let ev: Value = match serde_json::from_str(data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    match ev["type"].as_str().unwrap_or("") {
                        "message_start" => {
                            let usage = &ev["message"]["usage"];
                            input_tokens = usage["input_tokens"].as_u64().unwrap_or(0) as usize;
                            // Cache token fields are only present when prompt
                            // caching engaged on this call.  Treat absence as 0.
                            cache_creation_input_tokens = usage
                                ["cache_creation_input_tokens"]
                                .as_u64()
                                .unwrap_or(0) as usize;
                            cache_read_input_tokens = usage
                                ["cache_read_input_tokens"]
                                .as_u64()
                                .unwrap_or(0) as usize;
                        }

                        "content_block_start" => {
                            let cb = &ev["content_block"];
                            let btype = cb["type"].as_str().unwrap_or("").to_string();
                            match btype.as_str() {
                                "thinking" => {
                                    thinking_text.clear();
                                    thinking_sig.clear();
                                    // Empty initial chunk signals ThinkingStart to the consumer.
                                    yield Ok(Chunk::ThinkingSummary(String::new()));
                                }
                                "text" => {
                                    // Empty initial chunk signals start of text block.
                                    yield Ok(Chunk::Text(String::new()));
                                }
                                "tool_use" => {
                                    tool_id = cb["id"].as_str().unwrap_or("").to_string();
                                    tool_name = cb["name"].as_str().unwrap_or("").to_string();
                                    tool_args.clear();
                                }
                                _ => server.open(cb),
                            }
                            block_type = Some(btype);
                        }

                        "content_block_delta" => {
                            let delta = &ev["delta"];
                            match delta["type"].as_str().unwrap_or("") {
                                "thinking_delta" => {
                                    let t = delta["thinking"]
                                        .as_str()
                                        .unwrap_or("")
                                        .to_string();
                                    thinking_text.push_str(&t);
                                    yield Ok(Chunk::ThinkingSummary(t));
                                }
                                "signature_delta" => {
                                    thinking_sig.push_str(
                                        delta["signature"].as_str().unwrap_or(""),
                                    );
                                }
                                "text_delta" => {
                                    let t =
                                        delta["text"].as_str().unwrap_or("").to_string();
                                    yield Ok(Chunk::Text(t));
                                }
                                "input_json_delta" => {
                                    let part = delta["partial_json"].as_str().unwrap_or("");
                                    server.push_input(part, &mut tool_args);
                                }
                                _ => {}
                            }
                        }

                        "content_block_stop" => {
                            match block_type.as_deref() {
                                Some("thinking") => {
                                    let mut obj = serde_json::Map::new();
                                    obj.insert("type".into(), json!("thinking"));
                                    obj.insert(
                                        "thinking".into(),
                                        json!(thinking_text.clone()),
                                    );
                                    obj.insert(
                                        "signature".into(),
                                        json!(thinking_sig.clone()),
                                    );
                                    yield Ok(Chunk::RawBlock(ContentBlock::Thinking {
                                        provider_data: Value::Object(obj),
                                    }));
                                }
                                Some("tool_use") => {
                                    let input: Value =
                                        serde_json::from_str(&tool_args)
                                            .unwrap_or_else(|_| json!({}));
                                    yield Ok(Chunk::ToolCall(ToolCallChunk {
                                        id: tool_id.clone(),
                                        name: tool_name.clone(),
                                        input,
                                        provider_data: None,
                                    }));
                                }
                                _ => {
                                    if let Some(block) = server.close() {
                                        yield Ok(Chunk::RawBlock(block));
                                    }
                                }
                            }
                            block_type = None;
                        }

                        "message_delta" => {
                            let usage = &ev["usage"];
                            output_tokens =
                                usage["output_tokens"].as_u64().unwrap_or(0) as usize;
                            merge_delta_usage(usage, &mut input_tokens, &mut advisor);
                            // Anthropic occasionally re-reports cache tokens
                            // here; take max so a later 0 doesn't clobber a
                            // value seen at message_start.
                            raise_to(usage, "cache_creation_input_tokens", &mut cache_creation_input_tokens);
                            raise_to(usage, "cache_read_input_tokens", &mut cache_read_input_tokens);
                            if let Some(sr) = ev["delta"]["stop_reason"].as_str() {
                                stop_reason = parse_stop_reason(sr);
                            }
                        }

                        "message_stop" => {
                            yield Ok(Chunk::Done(Usage {
                                input_tokens,
                                output_tokens,
                                cache_creation_input_tokens,
                                cache_read_input_tokens,
                                stop_reason,
                                advisor,
                            }));
                            break 'outer;
                        }

                        _ => {}
                    }
                }
            }
        };

        Ok(Box::pin(s))
    }

    fn assistant_message(&self, blocks: &[ContentBlock]) -> Value {
        let content: Vec<Value> = blocks.iter().map(Self::block_to_wire).collect();
        json!({"role": "assistant", "content": content})
    }

    fn tool_result_messages(&self, results: &[(String, String, bool)]) -> Vec<Value> {
        let result_blocks: Vec<Value> = results
            .iter()
            .map(|(id, content, is_error)| {
                json!({
                    "type": "tool_result",
                    "tool_use_id": id,
                    "content": content,
                    "is_error": is_error
                })
            })
            .collect();
        vec![json!({"role": "user", "content": result_blocks})]
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn provider_name(&self) -> &str {
        self.provider_name
    }

    fn endpoint(&self) -> Option<&str> {
        Some(&self.base_url)
    }
}

#[cfg(test)]
mod tests;
