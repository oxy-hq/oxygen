use std::sync::Arc;

use serde_json::{Value, json};

use agentic_core::events::{CoreEvent, DomainEvents, Event, EventStream};

use super::constants::DEFAULT_MODEL;

/// Maximum number of characters included in the `llm.output` tracing preview.
/// Text beyond this limit is truncated with a count suffix to keep log lines
/// readable without losing all context for long outputs.
pub(crate) const LLM_OUTPUT_PREVIEW_MAX_CHARS: usize = 2000;
use super::genai::{self, GenAiContext, InferenceRequest};
use super::{
    AnthropicProvider, Chunk, ImageWire, LlmError, LlmProvider, OpenAiCompatProvider,
    ThinkingConfig, Usage,
};
use tracing::Instrument as _;

// ── LlmClient ─────────────────────────────────────────────────────────────────

/// High-level client that wraps an [`LlmProvider`] with tool-use loop logic
/// and event emission.
///
/// Construct with [`LlmClient::new`] (defaults to [`AnthropicProvider`]) or
/// supply a custom provider via [`LlmClient::with_provider`].
///
/// # Example
///
/// ```no_run
/// # use agentic_llm::LlmClient;
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let client = LlmClient::new(std::env::var("ANTHROPIC_API_KEY")?);
/// let answer = client.complete("You are helpful.", "What is 2 + 2?").await?;
/// println!("{answer}");
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct LlmClient {
    provider: Arc<dyn LlmProvider>,
    /// Stamped on every inference span — see [`GenAiContext`].
    genai: Arc<GenAiContext>,
    /// How the provider is sent pictures; `None` when it takes none
    /// (`crate::images`).
    image_wire: Option<ImageWire>,
}

impl LlmClient {
    /// Create a client backed by [`AnthropicProvider`] with the default model.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_provider(AnthropicProvider::new(api_key, DEFAULT_MODEL))
    }

    /// Create a client backed by [`AnthropicProvider`] with a custom model.
    pub fn with_model(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self::with_provider(AnthropicProvider::new(api_key, model))
    }

    /// Create a client backed by [`OpenAiCompatProvider`] (Chat Completions API).
    ///
    /// Use this for Ollama, vLLM, LM Studio, and any other OpenAI-compatible
    /// backend.  `base_url` should be the API root, e.g.
    /// `"http://localhost:11434/v1"`.
    pub fn with_openai_compat(
        api_key: impl Into<String>,
        model: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Self {
        Self::with_provider(OpenAiCompatProvider::new(api_key, model, base_url))
    }

    /// Create a client backed by a fully custom provider.
    pub fn with_provider(provider: impl LlmProvider + 'static) -> Self {
        Self {
            image_wire: ImageWire::of(&provider),
            provider: Arc::new(provider),
            genai: Arc::new(GenAiContext::default()),
        }
    }

    /// Attach who is asking, so every inference span carries the tenant,
    /// conversation and agent for per-tenant accounting in the platform
    /// trace store. Absent fields are simply not recorded.
    pub fn with_genai_context(mut self, ctx: GenAiContext) -> Self {
        self.genai = Arc::new(ctx);
        self
    }

    /// The context stamped on inference spans.
    pub fn genai_context(&self) -> &GenAiContext {
        &self.genai
    }

    /// The provider behind this client, for [`crate::engine::LlmEngine`],
    /// which renders messages and spans through it.
    pub(crate) fn provider(&self) -> &dyn LlmProvider {
        &*self.provider
    }

    /// Open the provider stream inside `span`, recording a request-level
    /// failure on it, and hand back the stream wrapped so the span also
    /// learns the response (usage, finish reason, error class).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn observed_stream(
        &self,
        span: &tracing::Span,
        system: &str,
        system_date_suffix: &str,
        messages: &[Value],
        tools: &[agentic_core::tools::ToolDef],
        thinking: &ThinkingConfig,
        response_schema: Option<&super::ResponseSchema>,
        max_tokens_override: Option<u32>,
    ) -> Result<genai::ChunkStream, LlmError> {
        // Every caller already runs inside `span`; this only opens and wraps.
        // The clock starts here, before the request leaves, so time to first
        // chunk includes connect, upload and the provider's queue.
        let started = std::time::Instant::now();
        let stream = self
            .provider
            .stream(
                system,
                system_date_suffix,
                messages,
                tools,
                thinking,
                response_schema,
                max_tokens_override,
            )
            .await
            .inspect_err(|e| genai::record_error(span, e))?;
        Ok(genai::observe(span.clone(), started, stream))
    }

    /// Build the message history for resuming after an `ask_user` suspension.
    ///
    /// `prior_messages` is the full provider-native message history returned
    /// inside [`LlmError::Suspended`] — it already contains the user turn,
    /// every prior tool round, and the assistant turn with the `ask_user` call.
    /// This function appends the tool result (the user's answer) and returns
    /// the complete history ready for [`InitialMessages::Messages`].
    ///
    /// Passing `prior_messages` ensures the LLM sees all tool calls it made
    /// before suspending, not just the ask_user exchange.
    pub fn build_resume_messages(
        &self,
        prior_messages: &[Value],
        question: &str,
        suggestions: &[String],
        answer: &str,
    ) -> Vec<Value> {
        tracing::trace!(
            prior_messages_count = prior_messages.len(),
            "build_resume_messages called"
        );
        let unmatched_ids = find_all_unmatched_tool_ids(prior_messages);
        let ids_to_resolve = if unmatched_ids.is_empty() {
            vec!["ask_user_0".to_string()]
        } else {
            unmatched_ids
        };

        let _ = (question, suggestions); // already encoded in prior_messages
        let mut msgs = prior_messages.to_vec();
        // For batched tool calls, put the full answer on the first result only;
        // subsequent results get "confirmed" to avoid repeating a long summary N times.
        let result_pairs: Vec<(String, String, bool)> = ids_to_resolve
            .into_iter()
            .enumerate()
            .map(|(i, id)| {
                let content = if i == 0 {
                    answer.to_string()
                } else {
                    "confirmed".to_string()
                };
                (id, content, false)
            })
            .collect();
        let new_results = self.provider.tool_result_messages(&result_pairs);

        // Anthropic requires all tool_results for a batch in one user message.
        // If the tool loop already flushed partial results before suspending,
        // merge new results into that message rather than appending a new one.
        if let Some(last) = msgs.last_mut()
            && last["role"].as_str() == Some("user")
            && let Some(content) = last["content"].as_array()
        {
            let has_tool_results = content
                .iter()
                .any(|b| b["type"].as_str() == Some("tool_result"));
            if has_tool_results {
                let mut merged_content = content.clone();
                for r in &new_results {
                    if let Some(blocks) = r["content"].as_array() {
                        merged_content.extend(blocks.iter().cloned());
                    } else {
                        debug_assert!(
                            false,
                            "tool_result_messages returned a result without a content array"
                        );
                    }
                }
                last["content"] = Value::Array(merged_content);
                return msgs;
            }
        }

        msgs.extend(new_results);
        msgs
    }

    /// Build the message history for resuming after a [`LlmError::MaxTokensReached`]
    /// or [`LlmError::MaxToolRoundsReached`] suspension.
    ///
    /// `prior_messages` is the full provider-native message history stored at
    /// the suspension point.  This function appends a user "please continue"
    /// message so the LLM resumes from where it left off.
    ///
    /// For `MaxTokensReached`: `prior_messages` already contains the truncated
    /// assistant turn; pass back with a doubled `max_tokens_override`.
    ///
    /// For `MaxToolRoundsReached`: `prior_messages` is the history up to the
    /// last tool results (before the model's unanswered tool request); pass
    /// back with an increased `max_tool_rounds`.
    pub fn build_continue_messages(prior_messages: &[Value]) -> Vec<Value> {
        let mut msgs = prior_messages.to_vec();
        msgs.push(json!({"role": "user", "content": "Please continue."}));
        msgs
    }

    /// Send a single-turn completion and return the assistant's text response.
    pub async fn complete(&self, system: &str, user: &str) -> Result<String, LlmError> {
        let (text, _) = self.complete_with_usage(system, user).await?;
        Ok(text)
    }

    /// Like [`complete`] but with an explicit `max_tokens` cap on the response.
    ///
    /// Useful for constrained outputs (e.g. asking the model to reply with a
    /// single letter).
    pub async fn complete_with_max_tokens(
        &self,
        system: &str,
        user: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        let messages = vec![json!({"role": "user", "content": user})];
        let span = genai::inference_span(
            &*self.provider,
            &self.genai,
            &InferenceRequest {
                system,
                messages: &messages,
                max_tokens: Some(max_tokens),
                tool_count: 0,
                structured_output: false,
                state: None,
                round: None,
            },
        );
        async {
            let s = self
                .observed_stream(
                    &span,
                    system,
                    "",
                    &messages,
                    &[],
                    &ThinkingConfig::Disabled,
                    None,
                    Some(max_tokens),
                )
                .await?;

            let mut text = String::new();
            let mut s = std::pin::pin!(s);
            while let Some(chunk) = {
                use tokio_stream::StreamExt as _;
                s.next().await
            } {
                if let Chunk::Text(t) = chunk? {
                    text.push_str(&t)
                }
            }

            if text.is_empty() {
                let err = LlmError::Parse("no text content in response".into());
                genai::record_error(&span, &err);
                return Err(err);
            }

            Ok(text)
        }
        .instrument(span.clone())
        .await
    }

    /// Like [`complete`] but also returns the [`Usage`] reported by the API.
    ///
    /// [`complete`]: LlmClient::complete
    pub async fn complete_with_usage(
        &self,
        system: &str,
        user: &str,
    ) -> Result<(String, Usage), LlmError> {
        let messages = vec![json!({"role": "user", "content": user})];
        let span = genai::inference_span(
            &*self.provider,
            &self.genai,
            &InferenceRequest {
                system,
                messages: &messages,
                max_tokens: None,
                tool_count: 0,
                structured_output: false,
                state: None,
                round: None,
            },
        );
        async {
            let s = self
                .observed_stream(
                    &span,
                    system,
                    "",
                    &messages,
                    &[],
                    &ThinkingConfig::Disabled,
                    None,
                    None,
                )
                .await?;

            let mut text = String::new();
            let mut usage = Usage::default();

            let mut s = std::pin::pin!(s);
            while let Some(chunk) = {
                use tokio_stream::StreamExt as _;
                s.next().await
            } {
                match chunk? {
                    Chunk::Text(t) => text.push_str(&t),
                    Chunk::Done(u) => usage = u,
                    _ => {}
                }
            }

            if text.is_empty() {
                let err = LlmError::Parse("no text content in response".into());
                genai::record_error(&span, &err);
                return Err(err);
            }

            tracing::info!(
                name: "llm.usage",
                is_visible = true,
                prompt_tokens = usage.input_tokens as i64,
                completion_tokens = usage.output_tokens as i64,
                total_tokens = (usage.input_tokens + usage.output_tokens) as i64,
                model = %self.provider.model_name(),
                stop_reason = %format!("{:?}", usage.stop_reason),
            );

            Ok((text, usage))
        }
        .instrument(span.clone())
        .await
    }
}

mod images;
mod tool_loop;

// ── Helpers ──────────────────────────────────────────────────────────────────

pub(super) async fn emit_core<Ev: DomainEvents>(tx: &Option<EventStream<Ev>>, event: CoreEvent) {
    if let Some(tx) = tx {
        let _ = tx.send(Event::Core(event)).await;
    }
}

/// Return all unmatched tool-call IDs from the most recent assistant turn, in
/// forward order.  IDs are returned for every tool_use that lacks a result —
/// needed when the LLM batches multiple calls and suspends on the first.
pub(crate) fn find_all_unmatched_tool_ids(messages: &[Value]) -> Vec<String> {
    let mut matched: std::collections::HashSet<String> = std::collections::HashSet::new();
    for m in messages.iter() {
        if m["role"].as_str() == Some("user")
            && let Some(content) = m["content"].as_array()
        {
            for b in content.iter() {
                if b["type"].as_str() == Some("tool_result")
                    && let Some(id) = b["tool_use_id"].as_str()
                {
                    matched.insert(id.to_string());
                }
            }
        }
        if m["role"].as_str() == Some("tool")
            && let Some(id) = m["tool_call_id"].as_str()
        {
            matched.insert(id.to_string());
        }
        if m["type"].as_str() == Some("function_call_output")
            && let Some(id) = m["call_id"].as_str()
        {
            matched.insert(id.to_string());
        }
        if let Some(items) = m.as_array() {
            for item in items.iter() {
                if item["type"].as_str() == Some("function_call_output")
                    && let Some(id) = item["call_id"].as_str()
                {
                    matched.insert(id.to_string());
                }
            }
        }
    }

    for m in messages.iter().rev() {
        let mut unmatched = Vec::new();
        if m["role"].as_str() == Some("assistant") {
            if let Some(blocks) = m["content"].as_array() {
                for b in blocks.iter() {
                    if b["type"].as_str() == Some("tool_use")
                        && let Some(id) = b["id"].as_str()
                        && !matched.contains(id)
                    {
                        unmatched.push(id.to_string());
                    }
                }
            }
            if let Some(tool_calls) = m["tool_calls"].as_array() {
                for tc in tool_calls.iter() {
                    if let Some(id) = tc["id"].as_str()
                        && !matched.contains(id)
                    {
                        unmatched.push(id.to_string());
                    }
                }
            }
            if !unmatched.is_empty() {
                return unmatched;
            }
        }
        // OpenAI Responses: flat function_call item
        if m["type"].as_str() == Some("function_call")
            && let Some(id) = m["call_id"].as_str()
            && !matched.contains(id)
        {
            return vec![id.to_string()];
        }
        // OpenAI Responses: array of items
        if let Some(items) = m.as_array() {
            for item in items.iter() {
                if item["type"].as_str() == Some("function_call")
                    && let Some(id) = item["call_id"].as_str()
                    && !matched.contains(id)
                {
                    unmatched.push(id.to_string());
                }
            }
            if !unmatched.is_empty() {
                return unmatched;
            }
        }
    }
    Vec::new()
}

/// Returns the first unmatched tool_use ID from the most recent assistant turn
/// — the tool that actually suspended.
#[cfg(test)]
pub(crate) fn find_suspended_tool_id(messages: &[Value]) -> Option<String> {
    find_all_unmatched_tool_ids(messages).into_iter().next()
}

/// Try to extract the first top-level JSON object from `text` that may be
/// surrounded by prose or markdown fences.  Returns `None` if no valid
/// object is found.
#[allow(dead_code)]
fn extract_json_object(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    // Walk from the end backwards to find the matching closing brace.
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str(&text[start..=end]).ok()
}
