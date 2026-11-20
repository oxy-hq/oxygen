//! [`LlmClient::run_with_tools`] — the core tool-use loop.

use std::time::Instant;

use serde_json::{Value, json};
use tracing::Instrument;

use agentic_core::events::{CoreEvent, DomainEvents, EventStream};
use agentic_core::tools::ToolDef;

use super::super::constants::{DEFAULT_MAX_TOKENS, THINKING_MAX_TOKENS};
use super::super::{
    Chunk, ContentBlock, InitialMessages, LlmError, LlmOutput, StopReason, ToolCallChunk,
    ToolLoopConfig, Usage,
};
use super::{LLM_OUTPUT_PREVIEW_MAX_CHARS, LlmClient, emit_core};

impl LlmClient {
    /// Run the LLM with an optional tool-use loop, emitting granular streaming
    /// events.
    ///
    /// # Event sequence
    ///
    /// ```text
    /// ┌─ per HTTP round ────────────────────────────────────────────┐
    /// │ LlmStart                        ← once per HTTP call        │
    /// │   ThinkingStart / ThinkingToken* / ThinkingEnd              │
    /// │   LlmToken*                                                 │
    /// │ LlmEnd                          ← once per HTTP call        │
    /// │   ToolCall / ToolResult          ← per tool in this round   │
    /// └─────────────────────────────────────────────────────────────┘
    /// … (repeat for each tool round)
    /// ```
    ///
    /// `LlmStart` / `LlmEnd` appear **once per HTTP round** (i.e. per
    /// `provider.stream()` call).  When the tool loop runs N rounds, there
    /// will be N `LlmStart`/`LlmEnd` pairs, each with per-round token
    /// counts and inference-only timing.  `ThinkingStart` / `ThinkingEnd`
    /// pairs may appear multiple times within a single round.
    ///
    /// # Thinking blobs
    ///
    /// Encrypted thinking blobs are preserved within the tool-use loop and
    /// returned in [`LlmOutput::raw_content_blocks`].  The caller (orchestrator)
    /// **must** discard them on every FSM state transition.
    ///
    /// # Tool executor
    ///
    /// `tool_executor` is an **async** closure.  Catalog-only tools wrap their
    /// synchronous result with `Box::pin(async move { sync_result })`.
    /// Connector-backed tools (e.g. `sample_column`, `execute_preview`) call
    /// `connector.execute_query(...).await` directly inside the box.
    pub async fn run_with_tools<Ev, F>(
        &self,
        system: &str,
        initial: impl Into<InitialMessages>,
        tools: &[ToolDef],
        mut tool_executor: F,
        events: &Option<EventStream<Ev>>,
        config: ToolLoopConfig,
    ) -> Result<LlmOutput, LlmError>
    where
        Ev: DomainEvents,
        F: FnMut(
            String,
            Value,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            Box<dyn agentic_core::tools::ToolOutput>,
                            agentic_core::tools::ToolError,
                        >,
                    > + Send,
            >,
        >,
    {
        let (mut messages, prompt_tokens) = match initial.into() {
            InitialMessages::User(user) => {
                let tokens = system.len() / 4 + user.len() / 4;
                (vec![json!({"role": "user", "content": user})], tokens)
            }
            InitialMessages::Messages(msgs) => {
                let tokens = system.len() / 4;
                (msgs, tokens)
            }
        };
        let ssi = config.sub_spec_index;

        let mut rounds: u32 = 0;
        let mut effective_max_tokens = config.max_tokens_override;
        // Estimated prompt tokens for the next LlmStart event.  Starts with
        // the initial estimate and is updated after each round to reflect the
        // growing conversation context.
        let mut next_prompt_tokens = prompt_tokens;
        // Collects text produced in rounds that also contain tool calls so that
        // all narrative output is preserved, not just the final round's text.
        let mut accumulated_text = String::new();
        // Accumulates every tool call made across all rounds.
        let mut all_tool_calls: Vec<(String, serde_json::Value)> = Vec::new();

        loop {
            let round_start = Instant::now();

            // Create a span for this LLM inference round.  The span
            // covers only the API call + stream consumption; it is dropped
            // before tool execution so its duration reflects pure inference
            // time.
            //
            // NOTE: Using `.instrument(span)` on an async block rather than
            // `let _guard = span.enter();` across `.await` points. The
            // `Entered` guard would be entered on one tokio worker thread and
            // dropped on another when the future migrates, corrupting
            // tracing-subscriber's per-thread span stack. The leaked span ids
            // then surfaced as `Span::current()` clone panics (ref_count==0)
            // in later sqlx queries on the coordinator — see the
            // cancel-race panic from `insert_event`.
            // One span per HTTP round, in both vocabularies: the product
            // store's `oxy.*` and the OTel GenAI conventions (`gen_ai.*`).
            let state_name = config.state.to_string();
            let llm_span = crate::genai::inference_span(
                &*self.provider,
                &self.genai,
                &crate::genai::InferenceRequest {
                    system,
                    messages: &messages,
                    max_tokens: effective_max_tokens,
                    tool_count: tools.len(),
                    structured_output: config.response_schema.is_some(),
                    state: Some(&state_name),
                    round: Some(rounds),
                },
            );

            #[allow(clippy::type_complexity)]
            let round_output: Result<
                (
                    String,
                    String,
                    Vec<ToolCallChunk>,
                    Vec<ContentBlock>,
                    Vec<ContentBlock>,
                    Usage,
                    u64,
                ),
                LlmError,
            > = async {
                // Emit LlmStart for every HTTP round so the frontend sees
                // each individual LLM call with its own token counts and
                // timing.
                emit_core(
                    events,
                    CoreEvent::LlmStart {
                        state: config.state.clone(),
                        prompt_tokens: next_prompt_tokens,
                        sub_spec_index: ssi,
                    },
                )
                .await;

                let s = self
                    .observed_stream(
                        &llm_span,
                        system,
                        config.system_date_hint.as_deref().unwrap_or(""),
                        &messages,
                        tools,
                        &config.thinking,
                        config.response_schema.as_ref(),
                        effective_max_tokens,
                    )
                    .await?;

                let mut text = String::new();
                let mut thinking_summary = String::new();
                let mut tool_calls: Vec<ToolCallChunk> = Vec::new();
                let mut raw_blocks: Vec<ContentBlock> = Vec::new();
                // Track the original stream order of reasoning items and
                // tool calls so we can reconstruct the assistant turn with
                // correct interleaving.  OpenAI requires each reasoning item
                // to be immediately followed by its output item
                // (function_call / message); re-ordering breaks that
                // invariant.
                let mut ordered_blocks: Vec<ContentBlock> = Vec::new();
                let mut in_thinking = false;
                let mut usage = Usage::default();
                let mut stream_err: Option<LlmError> = None;

                // The react engine folds a stream the same way in
                // `crate::engine::fold`; keep the two in step.
                let mut s = std::pin::pin!(s);
                loop {
                    use tokio_stream::StreamExt as _;
                    let Some(chunk) = s.next().await else { break };
                    match chunk {
                        Err(e) => {
                            stream_err = Some(e);
                            break;
                        }
                        Ok(chunk) => match chunk {
                            Chunk::ThinkingSummary(t) => {
                                if !in_thinking {
                                    in_thinking = true;
                                    emit_core(
                                        events,
                                        CoreEvent::ThinkingStart {
                                            state: config.state.clone(),
                                            sub_spec_index: ssi,
                                        },
                                    )
                                    .await;
                                }
                                thinking_summary.push_str(&t);
                                if !t.is_empty() {
                                    emit_core(
                                        events,
                                        CoreEvent::ThinkingToken {
                                            token: t,
                                            sub_spec_index: ssi,
                                        },
                                    )
                                    .await;
                                }
                            }
                            Chunk::Text(t) => {
                                if in_thinking {
                                    in_thinking = false;
                                    emit_core(
                                        events,
                                        CoreEvent::ThinkingEnd {
                                            state: config.state.clone(),
                                            sub_spec_index: ssi,
                                        },
                                    )
                                    .await;
                                }
                                text.push_str(&t);
                                // Suppress token streaming when the LLM is
                                // producing a structured JSON response via
                                // response_schema — the raw JSON is not
                                // user-friendly and the meaningful content
                                // is already captured in domain events
                                // (TriageCompleted, IntentClarified, etc.).
                                if !t.is_empty() && config.response_schema.is_none() {
                                    emit_core(
                                        events,
                                        CoreEvent::LlmToken {
                                            token: t,
                                            sub_spec_index: ssi,
                                        },
                                    )
                                    .await;
                                }
                            }
                            Chunk::ToolCall(tc) => {
                                // Destructure tc to move its fields.  One
                                // clone per field is still required because
                                // both ordered_blocks and tool_calls need
                                // owned copies.
                                let ToolCallChunk {
                                    id,
                                    name,
                                    input,
                                    provider_data,
                                } = tc;
                                ordered_blocks.push(ContentBlock::ToolUse {
                                    id: id.clone(),
                                    name: name.clone(),
                                    input: input.clone(),
                                    provider_data: provider_data.clone(),
                                });
                                tool_calls.push(ToolCallChunk {
                                    id,
                                    name,
                                    input,
                                    provider_data,
                                });
                            }
                            Chunk::RawBlock(block) => {
                                ordered_blocks.push(block.clone());
                                raw_blocks.push(block);
                            }
                            Chunk::Done(u) => {
                                usage = u;
                            }
                        },
                    }
                }

                // Close any open thinking block (stream ended mid-thinking).
                if in_thinking {
                    emit_core(
                        events,
                        CoreEvent::ThinkingEnd {
                            state: config.state.clone(),
                            sub_spec_index: ssi,
                        },
                    )
                    .await;
                }

                // Capture the LLM inference time for this round (excludes
                // tool execution).
                let round_llm_ms = round_start.elapsed().as_millis() as u64;

                // Record token usage as a visible span event on the LLM
                // span so it appears in the trace detail view.
                tracing::info!(
                    name: "llm.usage",
                    is_visible = true,
                    prompt_tokens = usage.input_tokens as i64,
                    completion_tokens = usage.output_tokens as i64,
                    total_tokens = (usage.input_tokens + usage.output_tokens) as i64,
                    cache_creation_tokens = usage.cache_creation_input_tokens as i64,
                    cache_read_tokens = usage.cache_read_input_tokens as i64,
                    model = %self.provider.model_name(),
                    duration_ms = round_llm_ms,
                    stop_reason = %format!("{:?}", usage.stop_reason),
                );

                // Record the LLM output (text + tool calls) as a visible
                // event.
                {
                    let tool_names: Vec<&str> =
                        tool_calls.iter().map(|tc| tc.name.as_str()).collect();
                    let output_preview_buf;
                    let output_preview: &str = if text.len() > LLM_OUTPUT_PREVIEW_MAX_CHARS {
                        // Truncate at a char boundary to avoid panicking on
                        // multi-byte UTF-8 characters (e.g. CJK, emoji) that
                        // may straddle the byte offset.
                        let truncate_at = text
                            .char_indices()
                            .take_while(|(i, _)| *i < LLM_OUTPUT_PREVIEW_MAX_CHARS)
                            .last()
                            .map(|(i, c)| i + c.len_utf8())
                            .unwrap_or(0);
                        output_preview_buf =
                            format!("{}… ({} chars)", &text[..truncate_at], text.len());
                        &output_preview_buf
                    } else {
                        &text
                    };
                    tracing::info!(
                        name: "llm.output",
                        is_visible = true,
                        text = %output_preview,
                        tool_calls = %serde_json::to_string(&tool_names).unwrap_or_default(),
                    );
                }

                // Emit LlmEnd for this round with per-round token counts
                // and inference-only timing (excludes tool execution).
                emit_core(
                    events,
                    CoreEvent::LlmEnd {
                        state: config.state.clone(),
                        output_tokens: usage.output_tokens,
                        input_tokens: 0,
                        cache_creation_input_tokens: usage.cache_creation_input_tokens,
                        cache_read_input_tokens: usage.cache_read_input_tokens,
                        duration_ms: round_llm_ms,
                        model: self.provider.model_name().to_string(),
                        sub_spec_index: ssi,
                    },
                )
                .await;

                // Propagate stream errors AFTER the balanced LlmEnd.
                if let Some(e) = stream_err {
                    return Err(e);
                }

                Ok((
                    text,
                    thinking_summary,
                    tool_calls,
                    raw_blocks,
                    ordered_blocks,
                    usage,
                    round_llm_ms,
                ))
            }
            .instrument(llm_span.clone())
            .await;

            // Close the LLM span before tool execution so its duration
            // reflects only inference time, not tool latency.
            drop(llm_span);

            let (
                text,
                thinking_summary,
                tool_calls,
                raw_blocks,
                ordered_blocks,
                usage,
                round_llm_ms,
            ) = round_output?;

            // Check if any tool call is the structured-response tool.
            // When found, treat it as the final output rather than executing it.
            if let Some(ref schema) = config.response_schema
                && let Some(schema_tc) = tool_calls.iter().find(|tc| tc.name == schema.name)
            {
                let structured = schema_tc.input.clone();
                let text_json =
                    serde_json::to_string(&structured).unwrap_or_else(|_| "{}".to_string());
                return Ok(LlmOutput {
                    text: text_json,
                    thinking_summary: if thinking_summary.is_empty() {
                        None
                    } else {
                        Some(thinking_summary)
                    },
                    raw_content_blocks: raw_blocks,
                    structured_response: Some(structured),
                    tool_calls: all_tool_calls,
                    prior_messages: messages,
                });
            }

            // No tool calls — final response.
            if tool_calls.is_empty() {
                // ── Empty-response guard ───────────────────────────────────────
                // The model produced thinking/reasoning but no text content.
                // Two sub-cases:
                //
                // 1. `MaxTokens` — the thinking phase consumed the entire
                //    token budget.  Retry with a doubled limit.
                // 2. Any other stop reason (e.g. `EndTurn`) — the model
                //    chose to stop without emitting text (common with
                //    OpenAI reasoning + structured output).  Retry once;
                //    if that also fails, return EmptyResponse.
                if text.trim().is_empty() && !thinking_summary.is_empty() {
                    if rounds < config.max_tool_rounds {
                        // The model produced thinking but no text.  Instead of
                        // discarding the thinking and retrying from scratch,
                        // preserve it by appending the assistant turn + a
                        // "Continue" user message.  This avoids wasting the
                        // thinking budget and nudges the model to emit text.
                        //
                        // Anthropic rejects assistant messages whose final
                        // block is a `thinking` block, so we append an empty
                        // text block as a sentinel to satisfy the constraint.
                        let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
                        assistant_blocks.extend(ordered_blocks);
                        assistant_blocks.push(ContentBlock::Text {
                            text: String::new(),
                        });
                        messages.push(self.provider.assistant_message(&assistant_blocks));
                        messages.push(json!({"role": "user", "content": "Continue."}));

                        if usage.stop_reason == StopReason::MaxTokens {
                            let current = effective_max_tokens.unwrap_or(THINKING_MAX_TOKENS);
                            effective_max_tokens = Some(current.saturating_mul(2));
                        }
                        next_prompt_tokens = usage.input_tokens + usage.output_tokens;
                        rounds += 1;
                        continue; // re-enter the outer provider.stream() loop
                    }
                    // Exhausted retries — return an error.
                    // LlmEnd was already emitted for this round above.
                    let reason = if usage.stop_reason == StopReason::MaxTokens {
                        "model hit max_tokens during thinking; retry also truncated"
                    } else {
                        "model produced reasoning but no text output; retries exhausted"
                    };
                    return Err(LlmError::EmptyResponse {
                        reason: reason.into(),
                    });
                }

                // Merge text from all prior tool-call rounds with this final round.
                // Insert a newline separator between accumulated and final-round
                // text when neither side is empty and there is no trailing newline.
                let text = if accumulated_text.is_empty() {
                    text
                } else if text.is_empty() {
                    accumulated_text
                } else {
                    if !accumulated_text.ends_with('\n') {
                        accumulated_text.push('\n');
                    }
                    accumulated_text.push_str(&text);
                    accumulated_text
                };

                // Text was truncated by the token limit — suspend so the caller
                // can ask the user whether to double the budget and continue.
                // (The thinking-only MaxTokens case is handled earlier above.)
                if usage.stop_reason == StopReason::MaxTokens {
                    // Append the (partial) assistant turn so the resume context
                    // includes what was generated before the cutoff.
                    let mut assistant_blocks = vec![ContentBlock::Text { text: text.clone() }];
                    assistant_blocks.extend(ordered_blocks);
                    messages.push(self.provider.assistant_message(&assistant_blocks));

                    // LlmEnd was already emitted for this round above.
                    return Err(LlmError::MaxTokensReached {
                        partial_text: text,
                        current_max_tokens: effective_max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
                        prior_messages: messages,
                    });
                }

                // LlmEnd was already emitted for this round above.

                // When a response_schema is configured and the model returned
                // plain JSON text (OpenAI response_format path), parse it into
                // structured_response so callers get the same shape as the
                // tool-call path.
                let structured_response = if config.response_schema.is_some() {
                    serde_json::from_str::<Value>(&text).ok()
                } else {
                    None
                };

                return Ok(LlmOutput {
                    text,
                    thinking_summary: if thinking_summary.is_empty() {
                        None
                    } else {
                        Some(thinking_summary)
                    },
                    raw_content_blocks: raw_blocks,
                    structured_response,
                    tool_calls: all_tool_calls,
                    prior_messages: messages,
                });
            }

            // Round-limit check before processing tool calls.
            // LlmEnd was already emitted for this round above.
            if rounds >= config.max_tool_rounds {
                return Err(LlmError::MaxToolRoundsReached {
                    rounds,
                    prior_messages: messages.clone(),
                });
            }

            // Append the full assistant turn in **stream order**.
            //
            // `ordered_blocks` preserves the exact interleaving of reasoning
            // items and function-call items as produced by the model.  OpenAI
            // requires each reasoning item to be immediately followed by its
            // output item; reordering (e.g. all reasoning first, then all
            // tool calls) breaks that invariant and causes:
            //   "Item '…' of type 'reasoning' was provided without its
            //    required following item."
            //
            // Text (accumulated from stream deltas) is prepended **before**
            // the interleaved reasoning/function_call items.  Anthropic
            // requires text content blocks to precede tool_use blocks in
            // the assistant message — placing text at the end causes:
            //   "tool_use ids were found without tool_result blocks
            //    immediately after"
            // OpenAI's assistant_message() independently flushes text parts
            // before each reasoning/function_call item, so the input order
            // of text relative to those items is handled correctly either way.
            let mut assistant_blocks = Vec::new();
            if !text.is_empty() {
                assistant_blocks.push(ContentBlock::Text { text: text.clone() });
            }
            assistant_blocks.extend(ordered_blocks);
            messages.push(self.provider.assistant_message(&assistant_blocks));

            let mut tool_results: Vec<(String, String, bool)> = Vec::new();
            for (tool_idx, tc) in tool_calls.iter().enumerate() {
                all_tool_calls.push((tc.name.clone(), tc.input.clone()));
                emit_core(
                    events,
                    CoreEvent::ToolCall {
                        name: tc.name.clone(),
                        input: tc.input.to_string(),
                        llm_duration_ms: round_llm_ms,
                        sub_spec_index: ssi,
                    },
                )
                .await;

                let t_tool = Instant::now();
                let exec_result = tool_executor(tc.name.clone(), tc.input.clone()).await;
                let tool_ms = t_tool.elapsed().as_millis() as u64;

                // Early-return for ask_user suspension.
                //
                // Before returning, flush any tool results collected so far
                // (from tools executed earlier in this batch).  Without this,
                // prior_messages would contain the assistant turn with ALL
                // function_calls but only the ask_user output — causing
                // "No tool output found for function call" from OpenAI.
                if let Err(agentic_core::tools::ToolError::Suspended {
                    ref prompt,
                    ref suggestions,
                }) = exec_result
                {
                    // Flush already-collected results for tools executed
                    // before ask_user in this batch (if any).
                    if !tool_results.is_empty() {
                        for msg in self.provider.tool_result_messages(&tool_results) {
                            messages.push(msg);
                        }
                    }

                    // Emit ToolCall events for tools that follow the suspended
                    // tool in this round — they were part of the LLM response
                    // but will not be executed this turn, so announce them now
                    // so the reasoning trace shows the full set of tool calls.
                    for remaining_tc in &tool_calls[tool_idx + 1..] {
                        all_tool_calls
                            .push((remaining_tc.name.clone(), remaining_tc.input.clone()));
                        emit_core(
                            events,
                            CoreEvent::ToolCall {
                                name: remaining_tc.name.clone(),
                                input: remaining_tc.input.to_string(),
                                llm_duration_ms: round_llm_ms,
                                sub_spec_index: ssi,
                            },
                        )
                        .await;
                    }

                    // LlmEnd was already emitted for this round above.
                    return Err(LlmError::Suspended {
                        prompt: prompt.clone(),
                        suggestions: suggestions.clone(),
                        prior_messages: messages.clone(),
                    });
                }

                let (llm_str, event_str, is_error) = match exec_result {
                    Ok(v) => {
                        let agent_text = v.to_agent_text();
                        let structured = v.to_value().to_string();
                        (agent_text, structured, false)
                    }
                    Err(e) => {
                        let s = e.to_string();
                        (s.clone(), s, true)
                    }
                };

                emit_core(
                    events,
                    CoreEvent::ToolResult {
                        name: tc.name.clone(),
                        output: event_str,
                        duration_ms: tool_ms,
                        is_error,
                        sub_spec_index: ssi,
                    },
                )
                .await;

                tool_results.push((tc.id.clone(), llm_str, is_error));
            }

            for msg in self.provider.tool_result_messages(&tool_results) {
                messages.push(msg);
            }

            // Save this round's text before the next iteration resets it.
            // Insert a newline separator when the running text doesn't already
            // end with one so that consecutive rounds read as distinct chunks.
            if !text.is_empty() {
                if !accumulated_text.is_empty() && !accumulated_text.ends_with('\n') {
                    accumulated_text.push('\n');
                }
                accumulated_text.push_str(&text);
            }
            rounds += 1;
            // Update prompt token estimate for the next round: use the
            // provider-reported input_tokens from this round as a base, plus
            // a rough estimate of the new tool result content.
            next_prompt_tokens = usage.input_tokens
                + usage.output_tokens
                + tool_results
                    .iter()
                    .map(|(_, r, _)| r.len() / 4)
                    .sum::<usize>();
        }
    }
}
