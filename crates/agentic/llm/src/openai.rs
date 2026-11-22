use std::collections::HashMap;
use std::pin::Pin;

use async_stream::stream;
use async_trait::async_trait;
use futures_core::Stream;
use serde_json::{Value, json};

use agentic_core::tools::ToolDef;

use super::constants::{
    OPENAI_BASE_URL, build_llm_http_client, parse_retry_after, send_error_to_llm,
};
use super::sse::{ApiError, pop_sse_event, sse_data, sse_event_type};
use super::{
    Chunk, ContentBlock, LlmError, LlmProvider, ReasoningEffort, ResponseSchema, StopReason,
    ThinkingConfig, ToolCallChunk, Usage,
};

// ── OpenAI schema helpers ─────────────────────────────────────────────────────

/// Recursively validate that a JSON Schema is compatible with OpenAI strict
/// mode (`"strict": true`).
///
/// OpenAI requires every key in `properties` to also appear in `required`.
/// Optional fields must be expressed as nullable (`{"type": ["T", "null"]}`)
/// rather than simply omitted from `required`.
///
/// Returns a list of human-readable violation strings; an empty list means
/// the schema is compliant.  Recurses into `anyOf` branches and `items`
/// schemas so deeply-nested objects are also validated.
pub fn validate_openai_strict_schema(schema: &Value, path: &str) -> Vec<String> {
    let mut out = Vec::new();
    validate_openai_strict_inner(schema, path, &mut out);
    out
}

fn validate_openai_strict_inner(schema: &Value, path: &str, out: &mut Vec<String>) {
    if schema.get("type").is_some_and(|t| t == "object")
        && let Some(props) = schema["properties"].as_object()
    {
        let required: Vec<&str> = schema["required"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        for key in props.keys() {
            if !required.contains(&key.as_str()) {
                out.push(format!(
                    "'{path}': property '{key}' is not in 'required' \
                         (mark it required and use {{\"type\": [\"T\", \"null\"]}} if optional)"
                ));
            }
            validate_openai_strict_inner(&props[key], &format!("{path}.{key}"), out);
        }
    }
    if let Some(branches) = schema.get("anyOf").and_then(|v| v.as_array()) {
        for (i, branch) in branches.iter().enumerate() {
            validate_openai_strict_inner(branch, &format!("{path}[anyOf:{i}]"), out);
        }
    }
    if let Some(items) = schema.get("items") {
        validate_openai_strict_inner(items, &format!("{path}[items]"), out);
    }
}

/// Recursively injects `"additionalProperties": false` into every object
/// schema in `val`.  OpenAI's strict-mode function calling requires this
/// field on every `{"type": "object"}` node.
pub fn inject_additional_properties_false(val: &mut Value) {
    if let Some(obj) = val.as_object_mut() {
        if obj.get("type").and_then(|v| v.as_str()) == Some("object") {
            obj.entry("additionalProperties").or_insert(json!(false));
        }
        for v in obj.values_mut() {
            inject_additional_properties_false(v);
        }
    } else if let Some(arr) = val.as_array_mut() {
        for item in arr {
            inject_additional_properties_false(item);
        }
    }
}

// ── OpenAiProvider ────────────────────────────────────────────────────────────

/// OpenAI Responses API provider (streaming).
///
/// Uses the `/v1/responses` endpoint which supports:
/// - **Reasoning continuity**: encrypted reasoning items are returned in the
///   stream and can be passed back verbatim during tool-use loops.
/// - **Reasoning effort**: [`ThinkingConfig::Effort`] maps to the `reasoning`
///   parameter with configurable effort level.
/// - **Reasoning summaries**: human-readable thinking text is streamed via
///   `response.reasoning_summary_text.delta` events.
pub struct OpenAiProvider {
    api_key: String,
    model: String,
    /// Root of the Responses API, e.g. `"https://api.openai.com/v1"`.
    /// The `/responses` path is appended by [`Self::responses_url`].
    base_url: String,
    /// `gen_ai.provider.name`, resolved once from the endpoint.
    provider_name: &'static str,
    /// Extra headers sent with every request, on top of the standard
    /// `content-type` / `Authorization` pair.
    headers: HashMap<String, String>,
    client: reqwest::Client,
}

impl OpenAiProvider {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: model.into(),
            provider_name: "openai",
            base_url: OPENAI_BASE_URL.to_string(),
            headers: HashMap::new(),
            client: build_llm_http_client(),
        }
    }

    /// Attach extra request headers, already resolved to literal values.
    /// Sent in addition to `content-type` and `Authorization`.
    pub fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = headers;
        self
    }

    /// Create a provider with a custom base URL.
    ///
    /// `base_url` should point to the root of the OpenAI-compatible API,
    /// e.g. `"https://api.openai.com/v1"`.  The `/responses` path is
    /// appended automatically, matching the behaviour of
    /// [`OpenAiCompatProvider`] which appends `/chat/completions`.
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
            provider_name: crate::genai::provider_name_for_url(&base, "openai"),
            base_url: base,
            headers: HashMap::new(),
            client: build_llm_http_client(),
        }
    }

    fn responses_url(&self) -> String {
        format!("{}/responses", self.base_url)
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn stream(
        &self,
        system: &str,
        system_date_suffix: &str,
        messages: &[Value],
        tools: &[ToolDef],
        thinking: &ThinkingConfig,
        response_schema: Option<&ResponseSchema>,
        _max_tokens_override: Option<u32>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Chunk, LlmError>> + Send>>, LlmError> {
        // OpenAI has no notion of system content blocks, so the date suffix
        // is concatenated onto the system string.  Acceptable since this
        // provider doesn't participate in prompt caching.
        let system_buf;
        let system: &str = if system_date_suffix.is_empty() {
            system
        } else {
            system_buf = format!("{system}\n{system_date_suffix}");
            &system_buf
        };
        // Build input items from the conversation history.
        // - Plain role messages get `"type": "message"` added.
        // - Arrays (from assistant_message) are flattened into individual items.
        // - Items that already have a `type` (e.g. function_call_output) pass through.
        // - Anthropic-format tool_use / tool_result blocks are converted to
        //   OpenAI Responses API function_call / function_call_output items.
        let mut input: Vec<Value> = Vec::new();
        for msg in messages {
            if let Some(arr) = msg.as_array() {
                input.extend(arr.iter().cloned());
            } else if msg.get("type").is_some() {
                input.push(msg.clone());
            } else if let Some(converted) = convert_anthropic_tool_msg(msg) {
                input.extend(converted);
            } else {
                let mut item = msg.clone();
                item["type"] = json!("message");
                input.push(item);
            }
        }
        let mut tools_json: Vec<Value> = tools
            .iter()
            .map(|t| {
                let mut params = t.parameters.clone();
                inject_additional_properties_false(&mut params);
                json!({
                    "type": "function",
                    "name": t.name,
                    "description": t.description,
                    "parameters": params,
                    "strict": true
                })
            })
            .collect();

        let mut body = json!({
            "model": self.model,
            "instructions": system,
            "input": input,
            "stream": true,
        });

        // Structured output: when no real tools, use text.format (constrained
        // decoding).  When real tools are present, inject a synthetic function
        // tool so the model can call it when done.
        if let Some(schema) = response_schema {
            if tools.is_empty() {
                let mut text_schema = schema.schema.clone();
                inject_additional_properties_false(&mut text_schema);
                body["text"] = json!({
                    "format": {
                        "type": "json_schema",
                        "name": schema.name,
                        "schema": text_schema,
                        "strict": true
                    }
                });
            } else {
                let mut schema_params = schema.schema.clone();
                inject_additional_properties_false(&mut schema_params);
                tools_json.push(json!({
                    "type": "function",
                    "name": schema.name,
                    "description": "Return the structured response.",
                    "parameters": schema_params,
                    "strict": true
                }));
            }
        }

        if !tools_json.is_empty() {
            body["tools"] = json!(tools_json);
        }

        // Reasoning configuration.  Only set for thinking-enabled configs;
        // `summary: "auto"` enables human-readable reasoning summaries in
        // the SSE stream.  Map Anthropic-specific configs: `Manual` →
        // `Effort(Medium)` so thinking is enabled regardless of flag.
        let effective_thinking = match thinking {
            ThinkingConfig::Manual { .. } => &ThinkingConfig::Effort(ReasoningEffort::Medium),
            other => other,
        };

        match effective_thinking {
            ThinkingConfig::Effort(effort) => {
                // `as_openai_str`, not `as_str`: the enum carries Anthropic's
                // five levels and this endpoint accepts three.
                body["reasoning"] = json!({
                    "effort": effort.as_openai_str(),
                    "summary": "auto"
                });
            }
            ThinkingConfig::Adaptive => {
                body["reasoning"] = json!({
                    "summary": "auto"
                });
            }
            ThinkingConfig::Disabled | ThinkingConfig::Manual { .. } => {}
        }

        let mut req = self
            .client
            .post(self.responses_url())
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json");
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
            // Tool call accumulators: output_index → (call_id, name, args)
            let mut tool_accum: std::collections::BTreeMap<u64, (String, String, String)> =
                std::collections::BTreeMap::new();
            let mut usage = Usage::default();

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

                    let ev_type = sse_event_type(&event_text);

                    let data = match sse_data(&event_text) {
                        Some(d) if !d.is_empty() => d,
                        _ => continue,
                    };

                    // Legacy [DONE] sentinel — not expected from the Responses
                    // API but handled for robustness.
                    if data == "[DONE]" {
                        break 'outer;
                    }

                    let ev: Value = match serde_json::from_str(data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    match ev_type.unwrap_or("") {
                        // ── Reasoning summary (human-readable thinking) ───────
                        "response.reasoning_summary_text.delta" => {
                            if let Some(delta) = ev["delta"].as_str() {
                                yield Ok(Chunk::ThinkingSummary(delta.to_string()));
                            }
                        }

                        // ── Text content delta ────────────────────────────────
                        "response.output_text.delta" => {
                            if let Some(delta) = ev["delta"].as_str()
                                && !delta.is_empty() {
                                    yield Ok(Chunk::Text(delta.to_string()));
                                }
                        }

                        // ── Function call argument delta ──────────────────────
                        "response.function_call_arguments.delta" => {
                            let idx = ev["output_index"].as_u64().unwrap_or(0);
                            let entry = tool_accum.entry(idx).or_insert_with(|| {
                                (String::new(), String::new(), String::new())
                            });
                            if let Some(delta) = ev["delta"].as_str() {
                                entry.2.push_str(delta);
                            }
                        }

                        // ── New output item started ───────────────────────────
                        "response.output_item.added" => {
                            let item = &ev["item"];
                            match item["type"].as_str().unwrap_or("") {
                                "function_call" => {
                                    let idx = ev["output_index"].as_u64().unwrap_or(0);
                                    let entry = tool_accum.entry(idx).or_insert_with(|| {
                                        (String::new(), String::new(), String::new())
                                    });
                                    if let Some(call_id) = item["call_id"].as_str() {
                                        entry.0 = call_id.to_string();
                                    }
                                    if let Some(name) = item["name"].as_str() {
                                        entry.1 = name.to_string();
                                    }
                                }
                                "reasoning" => {
                                    // Signal thinking block start.
                                    yield Ok(Chunk::ThinkingSummary(String::new()));
                                }
                                _ => {}
                            }
                        }

                        // ── Output item completed ─────────────────────────────
                        "response.output_item.done" => {
                            let item = &ev["item"];
                            match item["type"].as_str().unwrap_or("") {
                                "reasoning" => {
                                    // Emit the complete reasoning item as a
                                    // raw block.  Contains `encrypted_content`
                                    // that must be passed back verbatim during
                                    // tool-use loops.
                                    yield Ok(Chunk::RawBlock(ContentBlock::Thinking {
                                        provider_data: item.clone(),
                                    }));
                                }
                                "function_call" => {
                                    let idx = ev["output_index"].as_u64().unwrap_or(0);
                                    if let Some((call_id, name, args)) = tool_accum.remove(&idx) {
                                        let fn_input: Value =
                                            serde_json::from_str(&args).unwrap_or(Value::Null);
                                        // Store the complete native item for
                                        // verbatim passback in the next request.
                                        yield Ok(Chunk::ToolCall(ToolCallChunk {
                                            id: call_id,
                                            name,
                                            input: fn_input,
                                            provider_data: Some(item.clone()),
                                        }));
                                    }
                                }
                                _ => {}
                            }
                        }

                        // ── Response completed ────────────────────────────────
                        "response.completed" => {
                            if let Some(u) = ev.get("response")
                                .and_then(|r| r.get("usage"))
                                .or_else(|| ev.get("usage"))
                            {
                                usage = Usage {
                                    input_tokens: u["input_tokens"]
                                        .as_u64()
                                        .unwrap_or(0) as usize,
                                    output_tokens: u["output_tokens"]
                                        .as_u64()
                                        .unwrap_or(0) as usize,
                                    stop_reason: StopReason::EndTurn,
                                    ..Default::default()
                                };
                            }
                            // OpenAI: status "incomplete" with reason "max_output_tokens"
                            // signals the response was truncated.
                            if let Some(status) = ev.get("response").and_then(|r| r["status"].as_str())
                                && status == "incomplete" {
                                    let reason = ev["response"]["incomplete_details"]["reason"]
                                        .as_str()
                                        .unwrap_or("");
                                    if reason == "max_output_tokens" {
                                        usage.stop_reason = StopReason::MaxTokens;
                                    }
                                }
                            yield Ok(Chunk::Done(usage));
                            break 'outer;
                        }

                        _ => {}
                    }
                }
            }
        };

        Ok(Box::pin(s))
    }

    /// Serialise content blocks as Responses API input items.
    ///
    /// Returns a `Value::Array` containing individual output items (reasoning
    /// items, message items, function_call items).  The caller flattens this
    /// array into the `input` list when building the next request.
    fn assistant_message(&self, blocks: &[ContentBlock]) -> Value {
        let mut items: Vec<Value> = Vec::new();

        // Batch consecutive text blocks into a single message item.
        let mut text_parts: Vec<Value> = Vec::new();

        let flush_text = |parts: &mut Vec<Value>, out: &mut Vec<Value>| {
            if !parts.is_empty() {
                out.push(json!({
                    "type": "message",
                    "role": "assistant",
                    "content": std::mem::take(parts)
                }));
            }
        };

        for block in blocks {
            match block {
                ContentBlock::Thinking { provider_data } => {
                    flush_text(&mut text_parts, &mut items);
                    // Pass back the entire reasoning item verbatim
                    // (contains encrypted_content for continuity).
                    items.push(provider_data.clone());
                }
                ContentBlock::RedactedThinking { provider_data } => {
                    flush_text(&mut text_parts, &mut items);
                    items.push(provider_data.clone());
                }
                // Anthropic-only; this provider never produces one.
                ContentBlock::ServerTool { .. } => {}
                ContentBlock::Text { text } => {
                    text_parts.push(json!({
                        "type": "output_text",
                        "text": text
                    }));
                }
                ContentBlock::ToolUse {
                    id,
                    name,
                    input,
                    provider_data,
                } => {
                    flush_text(&mut text_parts, &mut items);
                    // Use the complete native item when available so all
                    // provider-specific fields (id, status, etc.) are
                    // preserved.  Fall back to reconstruction for items
                    // that don't carry provider_data (e.g. tests).
                    if let Some(native) = provider_data {
                        items.push(native.clone());
                    } else {
                        items.push(json!({
                            "type": "function_call",
                            "call_id": id,
                            "name": name,
                            "arguments": input.to_string()
                        }));
                    }
                }
            }
        }
        flush_text(&mut text_parts, &mut items);

        Value::Array(items)
    }

    fn tool_result_messages(&self, results: &[(String, String, bool)]) -> Vec<Value> {
        results
            .iter()
            .map(|(call_id, content, _is_error)| {
                json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": content
                })
            })
            .collect()
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

/// Convert an Anthropic-format message containing `tool_use` or `tool_result`
/// content blocks into OpenAI Responses API items (`function_call` /
/// `function_call_output`).  Returns `None` if the message doesn't contain
/// Anthropic tool blocks.
fn convert_anthropic_tool_msg(msg: &Value) -> Option<Vec<Value>> {
    let content = msg.get("content")?.as_array()?;
    let first_type = content.first()?.get("type")?.as_str()?;

    match first_type {
        "tool_use" => {
            // Assistant message with tool_use blocks →
            // function_call items for the Responses API.
            let items: Vec<Value> = content
                .iter()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
                .map(|b| {
                    json!({
                        "type": "function_call",
                        "call_id": b.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                        "name": b.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                        "arguments": b.get("input").map(|v| v.to_string()).unwrap_or_else(|| "{}".to_string())
                    })
                })
                .collect();
            Some(items)
        }
        "tool_result" => {
            // User message with tool_result blocks →
            // function_call_output items for the Responses API.
            let items: Vec<Value> = content
                .iter()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
                .map(|b| {
                    json!({
                        "type": "function_call_output",
                        "call_id": b.get("tool_use_id").and_then(|v| v.as_str()).unwrap_or(""),
                        "output": b.get("content").and_then(|v| v.as_str()).unwrap_or("")
                    })
                })
                .collect();
            Some(items)
        }
        _ => None,
    }
}

#[cfg(test)]
mod provider_tests {
    use super::*;

    #[test]
    fn responses_url_appends_to_custom_base() {
        let p = OpenAiProvider::with_base_url("k", "m", "https://gw.example/v1//");
        assert_eq!(p.responses_url(), "https://gw.example/v1/responses");
    }

    #[test]
    fn headers_default_empty_and_are_stored() {
        let p = OpenAiProvider::new("k", "m");
        assert!(p.headers.is_empty());
        let p = p.with_headers(HashMap::from([("x-gw".to_string(), "1".to_string())]));
        assert_eq!(p.headers["x-gw"], "1");
    }
}
