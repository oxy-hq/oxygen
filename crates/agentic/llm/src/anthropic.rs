use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};

use async_stream::stream;
use async_trait::async_trait;
use futures_core::Stream;
use serde_json::{Value, json};

use agentic_core::tools::ToolDef;

use super::constants::*;
use super::sse::{ApiError, pop_sse_event, sse_data};
use super::strict_schema::sanitize_for_strict;
use super::{
    Chunk, ContentBlock, LlmError, LlmProvider, ReasoningEffort, ResponseSchema, StopReason,
    ThinkingConfig, ToolCallChunk, Usage,
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

    fn block_to_wire(block: &ContentBlock) -> Value {
        match block {
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

        // Activate the extended-thinking beta whenever any thinking mode is
        // requested (including `Effort`, which `build_request_body` maps to
        // `Adaptive` in the body).  The mapping is centralised there; here we
        // only need to know "is thinking on at all?".
        if !matches!(thinking, ThinkingConfig::Disabled) {
            req = req.header("anthropic-beta", ANTHROPIC_THINKING_BETA);
        }
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
                                _ => {}
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
                                    tool_args.push_str(
                                        delta["partial_json"].as_str().unwrap_or(""),
                                    );
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
                                _ => {}
                            }
                            block_type = None;
                        }

                        "message_delta" => {
                            let usage = &ev["usage"];
                            output_tokens =
                                usage["output_tokens"].as_u64().unwrap_or(0) as usize;
                            // Anthropic occasionally re-reports cache tokens
                            // here; take max so a later 0 doesn't clobber a
                            // value seen at message_start.
                            if let Some(v) = usage["cache_creation_input_tokens"].as_u64()
                            {
                                cache_creation_input_tokens =
                                    cache_creation_input_tokens.max(v as usize);
                            }
                            if let Some(v) = usage["cache_read_input_tokens"].as_u64() {
                                cache_read_input_tokens =
                                    cache_read_input_tokens.max(v as usize);
                            }
                            // Parse stop_reason: "end_turn", "max_tokens", or "tool_use".
                            if let Some(sr) = ev["delta"]["stop_reason"].as_str() {
                                stop_reason = match sr {
                                    "max_tokens" => StopReason::MaxTokens,
                                    "tool_use" => StopReason::ToolUse,
                                    _ => StopReason::EndTurn,
                                };
                            }
                        }

                        "message_stop" => {
                            yield Ok(Chunk::Done(Usage {
                                input_tokens,
                                output_tokens,
                                cache_creation_input_tokens,
                                cache_read_input_tokens,
                                stop_reason,
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
mod tests {
    use super::*;

    fn body_for(model: &str, thinking: &ThinkingConfig) -> Value {
        AnthropicProvider::new("key", model).build_request_body(
            "sys",
            "",
            &[],
            &[],
            thinking,
            None,
            None,
        )
    }

    #[test]
    fn budget_tokens_classification_tracks_the_version_not_a_model_list() {
        // Still accepted.
        for m in [
            "claude-opus-4-6",
            "claude-sonnet-4-6",
            "claude-haiku-4-5-20251001",
            "claude-3-5-sonnet-20241022",
            // Claude 4.0: a major with NO minor segment, so the release date
            // sits where a minor would. Reading it as one classified these as
            // 4.20250514 -- i.e. "4.7 or later" -- and stripped a field they
            // accept in favour of adaptive thinking they do not support.
            "claude-sonnet-4-20250514",
            "claude-opus-4-20250514",
            "us.anthropic.claude-sonnet-4-20250514-v1:0",
            "claude-opus-4",
        ] {
            assert!(model_accepts_budget_tokens(m), "{m} should accept it");
        }
        // Removed on 4.7 and everything after, including vendor-prefixed and
        // suffixed spellings of the same model.
        for m in [
            "claude-opus-4-7",
            "claude-opus-4-8",
            "claude-sonnet-5",
            "claude-opus-5",
            "claude-fable-5",
            "claude-opus-5[1m]",
            "us.anthropic.claude-opus-4-7-v1:0",
            // A context-marker suffix on the MINOR token. No such id exists
            // yet, but a 1M variant of a 4.x model is the ordinary way this
            // function's "classifies a future model correctly" contract gets
            // tested -- and a whole-token length test would read this as 4.0
            // and hand it the budget_tokens shape 4.7 rejects.
            "claude-opus-4-7[1m]",
        ] {
            assert!(!model_accepts_budget_tokens(m), "{m} should reject it");
        }
        // Unknown ids keep the caller's config rather than guessing.
        assert!(model_accepts_budget_tokens("my-proxy-alias"));
    }

    #[test]
    fn a_dated_claude_4_id_keeps_its_budget_tokens_untranslated() {
        // End to end, not just the classifier: the shape that would have
        // regressed is a working request becoming a 400.
        let body = body_for(
            "claude-sonnet-4-20250514",
            &ThinkingConfig::Manual {
                budget_tokens: 8192,
            },
        );
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 8192);
        assert!(
            body["output_config"].get("effort").is_none(),
            "a 4.0 model must not be handed adaptive thinking plus effort"
        );
    }

    #[test]
    fn max_tokens_follows_the_thinking_config_actually_sent() {
        // A translated request carries no budget_tokens, so reserving
        // budget + DEFAULT for it would push max_tokens past the output cap
        // and 400 for a different reason than the one being fixed.
        let translated = body_for(
            "claude-sonnet-5",
            &ThinkingConfig::Manual {
                budget_tokens: 60_000,
            },
        );
        assert!(translated["thinking"].get("budget_tokens").is_none());
        assert_eq!(
            translated["max_tokens"], THINKING_MAX_TOKENS,
            "sized from the Effort it became, not the budget it no longer sends"
        );

        // On a model that keeps the budget, the headroom is still reserved.
        let kept = body_for(
            "claude-sonnet-4-20250514",
            &ThinkingConfig::Manual {
                budget_tokens: 60_000,
            },
        );
        assert_eq!(kept["max_tokens"], 60_000 + DEFAULT_MAX_TOKENS);
    }

    #[test]
    fn the_top_two_effort_levels_clamp_at_their_own_boundaries() {
        // `max` shipped on 4.6, `xhigh` on 4.7 -- so 4.6 is the one model where
        // the two answers differ, and clamping both at 4.7 would downgrade a
        // `max` that model accepts.
        let (xhigh, max) = (
            ThinkingConfig::Effort(ReasoningEffort::XHigh),
            ThinkingConfig::Effort(ReasoningEffort::Max),
        );

        assert_eq!(
            body_for("claude-opus-4-6", &xhigh)["output_config"]["effort"],
            "high",
            "xhigh postdates 4.6"
        );
        assert_eq!(
            body_for("claude-opus-4-6", &max)["output_config"]["effort"],
            "max",
            "max ships on 4.6 and must survive"
        );

        // Below 4.6 there is no adaptive thinking at all, so an Effort config
        // becomes a budget rather than a clamped level. Asserting a clamped
        // `effort` here would have read as "4.5 + effort works" while the
        // `thinking` field beside it 400'd.
        let old_model = body_for("claude-opus-4-5", &max);
        assert_eq!(old_model["thinking"]["type"], "enabled");
        assert_eq!(old_model["thinking"]["budget_tokens"], 16_384);
        assert!(old_model["output_config"].get("effort").is_none());

        // Above both.
        assert_eq!(
            body_for("claude-opus-5", &xhigh)["output_config"]["effort"],
            "xhigh"
        );
        assert_eq!(
            body_for("claude-opus-5", &max)["output_config"]["effort"],
            "max"
        );
    }

    #[test]
    fn adaptive_becomes_a_budget_on_a_model_that_predates_it() {
        // The pairing this repo's own template and config example ship:
        // `thinking: adaptive` on a 4.5 triage model. Before the arm existed,
        // `effort` on that model worked and a bare `adaptive` 400'd.
        let old_model = body_for("claude-haiku-4-5", &ThinkingConfig::Adaptive);
        assert_eq!(old_model["thinking"]["type"], "enabled");
        assert_eq!(old_model["thinking"]["budget_tokens"], 8_192);

        // 4.6 is where adaptive arrives, so it must pass straight through.
        let supported = body_for("claude-opus-4-6", &ThinkingConfig::Adaptive);
        assert_eq!(supported["thinking"]["type"], "adaptive");
        assert!(supported["thinking"].get("budget_tokens").is_none());
    }

    #[test]
    fn an_unrecognised_model_id_is_never_rewritten() {
        // Every probe answers "supported" for an id it cannot parse, so a proxy
        // alias or gateway keeps exactly the config the caller wrote. The
        // alternative -- assuming an unknown id is ancient -- silently
        // downgrades a request aimed at a model this code has not heard of.
        let alias = "my-gateway-alias";
        let xhigh = body_for(alias, &ThinkingConfig::Effort(ReasoningEffort::XHigh));
        assert_eq!(xhigh["output_config"]["effort"], "xhigh");
        let max = body_for(alias, &ThinkingConfig::Effort(ReasoningEffort::Max));
        assert_eq!(max["output_config"]["effort"], "max");
        let manual = body_for(
            alias,
            &ThinkingConfig::Manual {
                budget_tokens: 2048,
            },
        );
        assert_eq!(manual["thinking"]["type"], "enabled");
        assert_eq!(manual["thinking"]["budget_tokens"], 2048);

        // The fourth path: a bare `adaptive` must not be converted to a budget
        // just because the id is unparseable.
        let adaptive = body_for(alias, &ThinkingConfig::Adaptive);
        assert_eq!(adaptive["thinking"]["type"], "adaptive");
        assert!(adaptive["thinking"].get("budget_tokens").is_none());
    }

    #[test]
    fn effort_sends_adaptive_thinking_plus_output_config_effort() {
        // The whole point: before this, Effort collapsed to bare Adaptive and
        // the level was discarded, so there was no way to ask for less.
        let body = body_for(
            "claude-sonnet-5",
            &ThinkingConfig::Effort(ReasoningEffort::Low),
        );
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["output_config"]["effort"], "low");
    }

    #[test]
    fn effort_does_not_clobber_a_structured_output_format() {
        // `output_config` carries `format` for constrained decoding. Assigning
        // the effort object wholesale would silently drop it.
        let schema = ResponseSchema {
            name: "answer".to_string(),
            schema: json!({"type": "object"}),
        };
        let body = AnthropicProvider::new("key", "claude-sonnet-5").build_request_body(
            "sys",
            "",
            &[],
            &[],
            &ThinkingConfig::Effort(ReasoningEffort::Max),
            Some(&schema),
            None,
        );
        assert_eq!(body["output_config"]["effort"], "max");
        assert_eq!(body["output_config"]["format"]["type"], "json_schema");
    }

    #[test]
    fn budget_tokens_is_translated_on_a_model_that_rejects_it() {
        // The reported failure, verbatim: a `budget_tokens` config on sonnet 5
        // returned 400 "thinking.type.enabled is not supported for this
        // model", killing every call the agent made.
        let body = body_for(
            "claude-sonnet-5",
            &ThinkingConfig::Manual {
                budget_tokens: 2048,
            },
        );
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert!(
            body["thinking"].get("budget_tokens").is_none(),
            "the rejected field must not survive translation"
        );
        assert_eq!(body["output_config"]["effort"], "low");
    }

    #[test]
    fn budget_tokens_is_left_alone_on_a_model_that_still_takes_it() {
        let body = body_for(
            "claude-haiku-4-5-20251001",
            &ThinkingConfig::Manual {
                budget_tokens: 8192,
            },
        );
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 8192);
        assert!(body["output_config"].get("effort").is_none());
    }

    #[test]
    fn effort_gets_thinking_headroom_in_max_tokens() {
        // Effort is adaptive thinking with a depth hint, so it must not fall
        // into the small no-thinking default.
        let effort = body_for(
            "claude-sonnet-5",
            &ThinkingConfig::Effort(ReasoningEffort::Low),
        );
        let adaptive = body_for("claude-sonnet-5", &ThinkingConfig::Adaptive);
        let disabled = body_for("claude-sonnet-5", &ThinkingConfig::Disabled);
        assert_eq!(effort["max_tokens"], adaptive["max_tokens"]);
        assert!(
            effort["max_tokens"].as_u64() > disabled["max_tokens"].as_u64(),
            "thinking needs more headroom than a no-thinking request"
        );
    }

    #[test]
    fn disabled_sends_no_thinking_field() {
        let body = body_for("claude-sonnet-5", &ThinkingConfig::Disabled);
        assert!(body.get("thinking").is_none());
    }

    #[test]
    fn defaults_to_the_public_messages_endpoint() {
        let p = AnthropicProvider::new("key", "claude-sonnet-5");
        assert_eq!(p.messages_url(), "https://api.anthropic.com/v1/messages");
        assert!(p.headers.is_empty());
    }

    /// Regression guard. `AnthropicModelConfig::api_url` is
    /// `#[serde(default)]`-ed to the **root** `https://api.anthropic.com/v1`,
    /// so an omitted `api_url` reaches `build_llm_client` as `Some(root)`, not
    /// `None` — it always takes the `with_base_url` arm. While this provider
    /// POSTed `base_url` verbatim that sent every default Anthropic agent to
    /// `/v1` (404). Constructing from the config default must therefore land on
    /// exactly the same URL as constructing with no override at all.
    #[test]
    fn config_default_root_matches_the_no_override_url() {
        let from_config =
            AnthropicProvider::with_base_url("key", "m", "https://api.anthropic.com/v1");
        let from_default = AnthropicProvider::new("key", "m");
        assert_eq!(from_config.messages_url(), from_default.messages_url());
        assert_eq!(
            from_config.messages_url(),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn custom_root_gets_messages_appended() {
        let p = AnthropicProvider::with_base_url("key", "m", "https://proxy.internal/v1//");
        assert_eq!(p.messages_url(), "https://proxy.internal/v1/messages");
    }

    #[test]
    fn custom_headers_are_stored() {
        let p = AnthropicProvider::new("key", "m")
            .with_headers(HashMap::from([("x-gw".to_string(), "1".to_string())]));
        assert_eq!(p.headers["x-gw"], "1");
    }

    /// A tool schema with `minItems: 3` (the legacy specify schema's
    /// `join_path`) was sent as-is on a strict tool, and Anthropic answered
    /// 400 "For 'array' type, 'minItems' values other than 0 or 1 are not
    /// supported" -- for every request carrying it.
    fn join_path_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "join_path": {
                    "type": "array",
                    "items": {"type": "string"},
                    "minItems": 3,
                    "maxItems": 3
                }
            },
            "required": ["join_path"],
            "additionalProperties": false
        })
    }

    fn tool(name: &'static str, strict: bool) -> ToolDef {
        ToolDef {
            name,
            description: "d",
            parameters: join_path_schema(),
            strict,
            ..Default::default()
        }
    }

    fn body_with(tools: &[ToolDef], schema: Option<&ResponseSchema>) -> Value {
        AnthropicProvider::new("key", "claude-opus-4-8").build_request_body(
            "sys",
            "",
            &[],
            tools,
            &ThinkingConfig::Disabled,
            schema,
            None,
        )
    }

    #[test]
    fn strict_tool_schemas_are_reduced_to_the_strict_subset() {
        let body = body_with(&[tool("strict_one", true)], None);
        let jp = &body["tools"][0]["input_schema"]["properties"]["join_path"];
        assert_eq!(jp["minItems"], 1);
        assert!(jp.get("maxItems").is_none());
        assert_eq!(body["tools"][0]["strict"], true);
    }

    #[test]
    fn non_strict_tool_schemas_are_sent_unchanged() {
        // Outside strict mode the API accepts full JSON Schema, so the
        // constraint stays and the model sees it as written.
        let body = body_with(&[tool("loose", false)], None);
        assert_eq!(body["tools"][0]["input_schema"], join_path_schema());
    }

    #[test]
    fn structured_output_schemas_are_reduced_on_both_paths() {
        let schema = ResponseSchema {
            name: "specify_response".to_string(),
            schema: join_path_schema(),
        };
        // No real tools: sent as output_config.format.
        let body = body_with(&[], Some(&schema));
        let jp = &body["output_config"]["format"]["schema"]["properties"]["join_path"];
        assert_eq!(jp["minItems"], 1);
        // With real tools: sent as a synthetic strict tool.
        let body = body_with(&[tool("loose", false)], Some(&schema));
        let synthetic = &body["tools"][1];
        assert_eq!(synthetic["name"], "specify_response");
        assert_eq!(
            synthetic["input_schema"]["properties"]["join_path"]["minItems"],
            1
        );
    }
}
