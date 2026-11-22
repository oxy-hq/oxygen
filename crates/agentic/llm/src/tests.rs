use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use futures_core::Stream;
use serde_json::{Value, json};

use super::*;
use tokio::sync::mpsc;

use agentic_core::events::{CoreEvent, Event, EventStream};
use agentic_core::tools::{ToolDef, ToolOutput};

// ── Anthropic message validation helper ───────────────────────────────────

/// Validates Anthropic's constraint: every `tool_use` block in an
/// assistant message must be matched by a `tool_result` with the same
/// `tool_use_id` in the **immediately** following user message.
///
/// Returns `Ok(())` when the messages satisfy the constraint, or a
/// descriptive `Err(String)` on the first violation.
///
/// Checks both directions:
/// 1. Every `tool_use` in an assistant message must have a matching
///    `tool_result` in the immediately following user message.
/// 2. Every `tool_result` in a user message must have a matching
///    `tool_use` in the immediately preceding assistant message.
fn validate_anthropic_messages(messages: &[Value]) -> Result<(), String> {
    for (i, msg) in messages.iter().enumerate() {
        // Forward check: tool_use → tool_result in next message.
        if msg["role"].as_str() == Some("assistant") {
            let content = match msg["content"].as_array() {
                Some(arr) => arr,
                None => continue,
            };
            let tool_use_ids: Vec<&str> = content
                .iter()
                .filter(|b| b["type"].as_str() == Some("tool_use"))
                .filter_map(|b| b["id"].as_str())
                .collect();
            if tool_use_ids.is_empty() {
                continue;
            }
            let next = messages.get(i + 1).ok_or_else(|| {
                format!("messages.{i}: tool_use ids {tool_use_ids:?} with no following message")
            })?;
            if next["role"].as_str() != Some("user") {
                return Err(format!(
                    "messages.{i}: tool_use followed by role={:?}, expected 'user'",
                    next["role"]
                ));
            }
            let next_content = next["content"]
                .as_array()
                .ok_or_else(|| format!("messages.{}: content is not an array", i + 1))?;
            let result_ids: Vec<&str> = next_content
                .iter()
                .filter(|b| b["type"].as_str() == Some("tool_result"))
                .filter_map(|b| b["tool_use_id"].as_str())
                .collect();
            for tid in &tool_use_ids {
                if !result_ids.contains(tid) {
                    return Err(format!(
                        "messages.{i}: tool_use id '{tid}' has no matching tool_result \
                         in messages.{}. tool_use_ids={tool_use_ids:?}, result_ids={result_ids:?}",
                        i + 1
                    ));
                }
            }
        }

        // Reverse check: every tool_result in a user message must have a
        // matching tool_use in the immediately preceding assistant message.
        // This is the constraint the Anthropic API enforces with:
        //   "unexpected `tool_use_id` found in `tool_result` blocks".
        if msg["role"].as_str() == Some("user") {
            let content = match msg["content"].as_array() {
                Some(arr) => arr,
                None => continue,
            };
            let result_ids: Vec<&str> = content
                .iter()
                .filter(|b| b["type"].as_str() == Some("tool_result"))
                .filter_map(|b| b["tool_use_id"].as_str())
                .collect();
            if result_ids.is_empty() {
                continue;
            }
            let prev = if i == 0 {
                return Err(format!(
                    "messages.{i}: tool_result ids {result_ids:?} in first message \
                     (no preceding assistant message with tool_use)"
                ));
            } else {
                &messages[i - 1]
            };
            if prev["role"].as_str() != Some("assistant") {
                return Err(format!(
                    "messages.{i}: tool_result ids {result_ids:?} preceded by \
                     role={:?}, expected 'assistant'",
                    prev["role"]
                ));
            }
            let prev_content = prev["content"]
                .as_array()
                .ok_or_else(|| format!("messages.{}: content is not an array", i - 1))?;
            let use_ids: Vec<&str> = prev_content
                .iter()
                .filter(|b| b["type"].as_str() == Some("tool_use"))
                .filter_map(|b| b["id"].as_str())
                .collect();
            for rid in &result_ids {
                if !use_ids.contains(rid) {
                    return Err(format!(
                        "messages.{i}: tool_result id '{rid}' has no matching tool_use \
                         in messages.{}. result_ids={result_ids:?}, use_ids={use_ids:?}",
                        i - 1
                    ));
                }
            }
        }
    }
    Ok(())
}

// ── AnthropicMockProvider ─────────────────────────────────────────────────

/// A mock [`LlmProvider`] that uses **Anthropic-style** message formatting
/// (matching [`AnthropicProvider`]) and captures the `messages` array sent
/// to each [`stream`] call so tests can validate the conversation structure
/// without a network round-trip.
struct AnthropicMockProvider {
    rounds: Mutex<VecDeque<Vec<Result<Chunk, LlmError>>>>,
    captured: Arc<Mutex<Vec<Vec<Value>>>>,
}

impl AnthropicMockProvider {
    fn new(
        rounds: Vec<Vec<Result<Chunk, LlmError>>>,
        captured: Arc<Mutex<Vec<Vec<Value>>>>,
    ) -> Self {
        Self {
            rounds: Mutex::new(rounds.into()),
            captured,
        }
    }

    /// Identical to [`AnthropicProvider::block_to_wire`].
    fn block_to_wire(block: &ContentBlock) -> Value {
        match block {
            ContentBlock::Thinking { provider_data } => provider_data.clone(),
            ContentBlock::RedactedThinking { provider_data } => provider_data.clone(),
            ContentBlock::ServerTool { provider_data } => provider_data.clone(),
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

#[async_trait::async_trait]
impl LlmProvider for AnthropicMockProvider {
    async fn stream(
        &self,
        _system: &str,
        _system_date_suffix: &str,
        messages: &[Value],
        _tools: &[ToolDef],
        _thinking: &ThinkingConfig,
        _response_schema: Option<&ResponseSchema>,
        _max_tokens_override: Option<u32>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Chunk, LlmError>> + Send>>, LlmError> {
        self.captured.lock().unwrap().push(messages.to_vec());
        let chunks = self.rounds.lock().unwrap().pop_front().unwrap_or_default();
        Ok(Box::pin(tokio_stream::iter(chunks)))
    }

    /// Anthropic-style: all blocks serialised into one assistant message.
    fn assistant_message(&self, blocks: &[ContentBlock]) -> Value {
        let content: Vec<Value> = blocks.iter().map(Self::block_to_wire).collect();
        json!({"role": "assistant", "content": content})
    }

    /// Anthropic-style: all results batched into a single user message.
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
        "mock-anthropic"
    }
}

// ── MockProvider ──────────────────────────────────────────────────────────

/// A mock [`LlmProvider`] that returns pre-defined [`Chunk`] sequences.
///
/// Each call to [`stream`] pops one `Vec<Chunk>` from the front of the
/// queue.  If the queue is empty, returns an empty stream.
struct MockProvider {
    rounds: Mutex<VecDeque<Vec<Result<Chunk, LlmError>>>>,
}

impl MockProvider {
    fn new(rounds: Vec<Vec<Result<Chunk, LlmError>>>) -> Self {
        Self {
            rounds: Mutex::new(rounds.into()),
        }
    }
}

#[async_trait::async_trait]
impl LlmProvider for MockProvider {
    async fn stream(
        &self,
        _system: &str,
        _system_date_suffix: &str,
        _messages: &[Value],
        _tools: &[ToolDef],
        _thinking: &ThinkingConfig,
        _response_schema: Option<&ResponseSchema>,
        _max_tokens_override: Option<u32>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Chunk, LlmError>> + Send>>, LlmError> {
        let chunks = self.rounds.lock().unwrap().pop_front().unwrap_or_default();
        Ok(Box::pin(tokio_stream::iter(chunks)))
    }

    fn assistant_message(&self, _blocks: &[ContentBlock]) -> Value {
        json!({"role": "assistant", "content": []})
    }

    fn tool_result_messages(&self, results: &[(String, String, bool)]) -> Vec<Value> {
        results
            .iter()
            .map(|(id, content, _)| {
                json!({
                    "role": "tool",
                    "tool_call_id": id,
                    "content": content
                })
            })
            .collect()
    }

    fn model_name(&self) -> &str {
        "mock-openai"
    }
}

// Helper: collect events from channel after closing sender.
async fn drain_events(mut rx: mpsc::Receiver<Event<()>>) -> Vec<String> {
    let mut tags = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        if let Event::Core(core) = ev {
            let tag = match core {
                CoreEvent::LlmStart { .. } => "LlmStart",
                CoreEvent::LlmToken { .. } => "LlmToken",
                CoreEvent::LlmEnd { .. } => "LlmEnd",
                CoreEvent::ThinkingStart { .. } => "ThinkingStart",
                CoreEvent::ThinkingToken { .. } => "ThinkingToken",
                CoreEvent::ThinkingEnd { .. } => "ThinkingEnd",
                CoreEvent::ToolCall { .. } => "ToolCall",
                CoreEvent::ToolResult { .. } => "ToolResult",
                _ => "Other",
            };
            tags.push(tag.to_string());
        }
    }
    tags
}

// ── Test 1: ThinkingSummary → Text → Done ─────────────────────────────────

#[tokio::test]
async fn thinking_then_text_emits_correct_events() {
    let provider = MockProvider::new(vec![vec![
        Ok(Chunk::ThinkingSummary(String::new())), // block start signal
        Ok(Chunk::ThinkingSummary("I need to think.".into())),
        Ok(Chunk::ThinkingSummary(" More reasoning.".into())),
        Ok(Chunk::RawBlock(ContentBlock::Thinking {
            provider_data: json!({"type":"thinking","thinking":"...","signature":"sig"}),
        })),
        Ok(Chunk::Text(String::new())), // text block start signal
        Ok(Chunk::Text("Hello ".into())),
        Ok(Chunk::Text("world".into())),
        Ok(Chunk::Done(Usage {
            input_tokens: 10,
            output_tokens: 20,
            ..Default::default()
        })),
    ]]);

    let client = LlmClient::with_provider(provider);
    let (tx, rx) = mpsc::channel::<Event<()>>(64);
    let events: Option<EventStream<()>> = Some(tx);

    let output = client
        .run_with_tools::<(), _>(
            "system",
            "user",
            &[],
            |_, _| Box::pin(async { Ok(Box::new(Value::Null) as Box<dyn ToolOutput>) }),
            &events,
            ToolLoopConfig {
                state: "solving".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(output.text, "Hello world");
    assert_eq!(
        output.thinking_summary.as_deref(),
        Some("I need to think. More reasoning.")
    );

    drop(events);
    let tags = drain_events(rx).await;

    assert_eq!(
        tags,
        vec![
            "LlmStart",
            "ThinkingStart",
            "ThinkingToken",
            "ThinkingToken",
            "ThinkingEnd",
            "LlmToken",
            "LlmToken",
            "LlmEnd",
        ],
        "event order mismatch: {tags:?}"
    );
}

// ── Test 2: Tool call then second round ───────────────────────────────────

#[tokio::test]
async fn tool_call_round_then_final_text() {
    let provider = MockProvider::new(vec![
        // Round 1: tool call
        vec![
            Ok(Chunk::Text("Checking ".into())),
            Ok(Chunk::ToolCall(ToolCallChunk {
                id: "tc1".into(),
                name: "dry_run".into(),
                input: json!({"sql": "SELECT 1"}),
                provider_data: None,
            })),
            Ok(Chunk::Done(Usage {
                input_tokens: 10,
                output_tokens: 5,
                ..Default::default()
            })),
        ],
        // Round 2: final text
        vec![
            Ok(Chunk::Text("Done.".into())),
            Ok(Chunk::Done(Usage {
                input_tokens: 15,
                output_tokens: 3,
                ..Default::default()
            })),
        ],
    ]);

    let client = LlmClient::with_provider(provider);
    let (tx, rx) = mpsc::channel::<Event<()>>(64);
    let events: Option<EventStream<()>> = Some(tx);

    let mut executed: Vec<String> = Vec::new();

    let output = client
        .run_with_tools(
            "system",
            "user",
            &[],
            |name: String, _| {
                executed.push(name);
                Box::pin(async { Ok(Box::new(json!({"valid": true})) as Box<dyn ToolOutput>) })
            },
            &events,
            ToolLoopConfig {
                max_tool_rounds: 5,
                state: "solving".into(),
                thinking: ThinkingConfig::Disabled,
                response_schema: None,
                max_tokens_override: None,
                sub_spec_index: None,
                system_date_hint: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(output.text, "Checking \nDone.");
    assert_eq!(executed, vec!["dry_run"]);

    drop(events);
    let tags = drain_events(rx).await;

    assert_eq!(
        tags,
        vec![
            "LlmStart",
            "LlmToken", // "Checking "
            "LlmEnd",   // round 1 end
            "ToolCall",
            "ToolResult",
            "LlmStart", // round 2 start
            "LlmToken", // "Done."
            "LlmEnd",   // round 2 end
        ],
        "event order mismatch: {tags:?}"
    );
}

// ── Test 3: Interleaved thinking (ThinkingStart/End pairs twice) ──────────

#[tokio::test]
async fn interleaved_thinking_emits_two_thinking_pairs() {
    let provider = MockProvider::new(vec![
        // Round 1: thinking → tool call
        vec![
            Ok(Chunk::ThinkingSummary(String::new())),
            Ok(Chunk::ThinkingSummary("Round 1 thought.".into())),
            Ok(Chunk::RawBlock(ContentBlock::Thinking {
                provider_data: json!({"type":"thinking","thinking":"...","signature":"s1"}),
            })),
            Ok(Chunk::ToolCall(ToolCallChunk {
                id: "tc1".into(),
                name: "search_catalog".into(),
                input: json!({"queries": ["rev"]}),
                provider_data: None,
            })),
            Ok(Chunk::Done(Usage::default())),
        ],
        // Round 2: thinking → final text
        vec![
            Ok(Chunk::ThinkingSummary(String::new())),
            Ok(Chunk::ThinkingSummary("Round 2 thought.".into())),
            Ok(Chunk::RawBlock(ContentBlock::Thinking {
                provider_data: json!({"type":"thinking","thinking":"...","signature":"s2"}),
            })),
            Ok(Chunk::Text("Final answer.".into())),
            Ok(Chunk::Done(Usage {
                input_tokens: 20,
                output_tokens: 10,
                ..Default::default()
            })),
        ],
    ]);

    let client = LlmClient::with_provider(provider);
    let (tx, rx) = mpsc::channel::<Event<()>>(64);
    let events: Option<EventStream<()>> = Some(tx);

    let output = client
        .run_with_tools(
            "system",
            "user",
            &[],
            |_, _| Box::pin(async { Ok(Box::new(json!([])) as Box<dyn ToolOutput>) }),
            &events,
            ToolLoopConfig {
                max_tool_rounds: 5,
                state: "solving".into(),
                thinking: ThinkingConfig::Adaptive,
                response_schema: None,
                max_tokens_override: None,
                sub_spec_index: None,
                system_date_hint: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(output.text, "Final answer.");

    drop(events);
    let tags = drain_events(rx).await;

    // ThinkingStart/End must appear twice — once per round.
    let thinking_starts = tags
        .iter()
        .filter(|t| t.as_str() == "ThinkingStart")
        .count();
    let thinking_ends = tags.iter().filter(|t| t.as_str() == "ThinkingEnd").count();
    assert_eq!(
        thinking_starts, 2,
        "expected 2 ThinkingStart events: {tags:?}"
    );
    assert_eq!(thinking_ends, 2, "expected 2 ThinkingEnd events: {tags:?}");
}

// ── Test 4: Mid-stream error — LlmEnd still fires ─────────────────────────

#[tokio::test]
async fn stream_error_emits_llm_end_before_propagating() {
    let provider = MockProvider::new(vec![vec![
        Ok(Chunk::Text("partial".into())),
        Err(LlmError::Http("connection reset".into())),
    ]]);

    let client = LlmClient::with_provider(provider);
    let (tx, rx) = mpsc::channel::<Event<()>>(64);
    let events: Option<EventStream<()>> = Some(tx);

    let result = client
        .run_with_tools::<(), _>(
            "system",
            "user",
            &[],
            |_, _| Box::pin(async { Ok(Box::new(Value::Null) as Box<dyn ToolOutput>) }),
            &events,
            ToolLoopConfig {
                state: "solving".into(),
                ..Default::default()
            },
        )
        .await;

    assert!(
        matches!(result, Err(LlmError::Http(_))),
        "expected Http error, got {result:?}"
    );

    drop(events);
    let tags = drain_events(rx).await;

    assert!(
        tags.contains(&"LlmStart".to_string()),
        "LlmStart missing: {tags:?}"
    );
    assert!(
        tags.contains(&"LlmEnd".to_string()),
        "LlmEnd missing after error: {tags:?}"
    );
}

// ── Test 5: MaxToolRoundsReached ──────────────────────────────────────────

#[tokio::test]
async fn max_tool_rounds_exceeded() {
    // Every round returns a tool call — loop never terminates naturally.
    let rounds: Vec<Vec<Result<Chunk, LlmError>>> = (0..10)
        .map(|i| {
            vec![
                Ok(Chunk::ToolCall(ToolCallChunk {
                    id: format!("id_{i}"),
                    name: "search_catalog".into(),
                    input: json!({"queries": ["rev"]}),
                    provider_data: None,
                })),
                Ok(Chunk::Done(Usage::default())),
            ]
        })
        .collect();

    let provider = MockProvider::new(rounds);
    let client = LlmClient::with_provider(provider);

    let result = client
        .run_with_tools::<(), _>(
            "system",
            "user",
            &[],
            |_, _| Box::pin(async { Ok(Box::new(json!([])) as Box<dyn ToolOutput>) }),
            &None,
            ToolLoopConfig {
                max_tool_rounds: 3,
                state: "clarifying".into(),
                thinking: ThinkingConfig::Disabled,
                response_schema: None,
                max_tokens_override: None,
                sub_spec_index: None,
                system_date_hint: None,
            },
        )
        .await;

    assert!(
        matches!(
            result,
            Err(LlmError::MaxToolRoundsReached { rounds: 3, .. })
        ),
        "expected MaxToolRoundsReached(3), got {result:?}"
    );
}

// ── Test 6: Blob preservation — raw_blocks passed back on continuation ────

#[tokio::test]
async fn thinking_raw_blocks_preserved_in_tool_continuation() {
    let sig_blob = json!({
        "type": "thinking",
        "thinking": "I should call the tool.",
        "signature": "sig_abc123"
    });

    let provider = MockProvider::new(vec![
        // Round 1: thinking + tool call
        vec![
            Ok(Chunk::ThinkingSummary("I should call the tool.".into())),
            Ok(Chunk::RawBlock(ContentBlock::Thinking {
                provider_data: sig_blob.clone(),
            })),
            Ok(Chunk::ToolCall(ToolCallChunk {
                id: "tc1".into(),
                name: "dry_run".into(),
                input: json!({"sql": "SELECT 1"}),
                provider_data: None,
            })),
            Ok(Chunk::Done(Usage::default())),
        ],
        // Round 2: final text (no more thinking)
        vec![
            Ok(Chunk::Text("Revenue is high.".into())),
            Ok(Chunk::Done(Usage {
                input_tokens: 30,
                output_tokens: 5,
                ..Default::default()
            })),
        ],
    ]);

    let client = LlmClient::with_provider(provider);

    let output = client
        .run_with_tools::<(), _>(
            "system",
            "user",
            &[],
            |_, _| Box::pin(async { Ok(Box::new(json!({"valid": true})) as Box<dyn ToolOutput>) }),
            &None,
            ToolLoopConfig {
                max_tool_rounds: 5,
                state: "solving".into(),
                thinking: ThinkingConfig::Adaptive,
                response_schema: None,
                max_tokens_override: None,
                sub_spec_index: None,
                system_date_hint: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(output.text, "Revenue is high.");
    // Final round had no thinking, so summary is None.
    assert!(output.thinking_summary.is_none());
    // Final round raw_content_blocks contains only the text block.
    assert!(
        output.raw_content_blocks.iter().all(|b| {
            !matches!(
                b,
                ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. }
            )
        }),
        "orchestrator should see no thinking blobs in final LlmOutput"
    );
}

// ── Test 7: Per-state ThinkingConfig ──────────────────────────────────────

#[test]
fn each_state_can_have_its_own_thinking_config() {
    let clarifying = ToolLoopConfig {
        max_tool_rounds: 5,
        state: "clarifying".into(),
        thinking: ThinkingConfig::Disabled,
        response_schema: None,
        max_tokens_override: None,
        sub_spec_index: None,
        system_date_hint: None,
    };
    let solving = ToolLoopConfig {
        max_tool_rounds: 3,
        state: "solving".into(),
        thinking: ThinkingConfig::Adaptive,
        response_schema: None,
        max_tokens_override: None,
        sub_spec_index: None,
        system_date_hint: None,
    };
    let interpreting = ToolLoopConfig {
        max_tool_rounds: 2,
        state: "interpreting".into(),
        thinking: ThinkingConfig::Disabled,
        response_schema: None,
        max_tokens_override: None,
        sub_spec_index: None,
        system_date_hint: None,
    };

    assert!(matches!(clarifying.thinking, ThinkingConfig::Disabled));
    assert!(matches!(solving.thinking, ThinkingConfig::Adaptive));
    assert!(matches!(interpreting.thinking, ThinkingConfig::Disabled));
}

// ── Test 8: Backpressure — slow consumer doesn't panic ────────────────────

#[tokio::test]
async fn slow_consumer_does_not_panic() {
    // Small channel buffer to simulate backpressure.
    let (tx, mut rx) = mpsc::channel::<Event<()>>(2);
    let events: Option<EventStream<()>> = Some(tx);

    let chunks: Vec<Result<Chunk, LlmError>> = (0..50)
        .map(|i| Ok(Chunk::Text(format!("token{i} "))))
        .chain(std::iter::once(Ok(Chunk::Done(Usage {
            input_tokens: 10,
            output_tokens: 50,
            ..Default::default()
        }))))
        .collect();

    let provider = MockProvider::new(vec![chunks]);
    let client = LlmClient::with_provider(provider);

    // Spawn a slow consumer that reads one event at a time.
    tokio::spawn(async move {
        while let Some(_) = rx.recv().await {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    });

    let result = client
        .run_with_tools::<(), _>(
            "system",
            "user",
            &[],
            |_, _| Box::pin(async { Ok(Box::new(Value::Null) as Box<dyn ToolOutput>) }),
            &events,
            ToolLoopConfig {
                state: "solving".into(),
                ..Default::default()
            },
        )
        .await;

    // Should complete without panic regardless of backpressure.
    assert!(result.is_ok(), "expected Ok, got {result:?}");
}

// ── Test 9: Structured output — Anthropic tool-intercept path ─────────────
//
// When `response_schema` is set and the model emits a ToolCall whose name
// matches the schema name, `run_with_tools` must:
//   • NOT execute the schema tool (it is the response, not an action),
//   • return immediately with `structured_response` populated,
//   • set `text` to the JSON serialisation of the tool input.

#[tokio::test]
async fn structured_response_anthropic_schema_tool_intercepted() {
    let schema_input = json!({
        "question_type": "Trend",
        "metrics": ["revenue"],
        "dimensions": ["month"],
        "filters": []
    });

    let provider = MockProvider::new(vec![vec![
        Ok(Chunk::ToolCall(ToolCallChunk {
            id: "tc_schema".into(),
            name: "clarify_response".into(),
            input: schema_input.clone(),
            provider_data: None,
        })),
        Ok(Chunk::Done(Usage {
            input_tokens: 20,
            output_tokens: 30,
            ..Default::default()
        })),
    ]]);

    let client = LlmClient::with_provider(provider);
    let mut tool_executor_called = false;

    let output = client
        .run_with_tools::<(), _>(
            "system",
            "user",
            &[],
            |_: String, _| {
                tool_executor_called = true;
                Box::pin(async { Ok(Box::new(Value::Null) as Box<dyn ToolOutput>) })
            },
            &None,
            ToolLoopConfig {
                state: "clarifying".into(),
                response_schema: Some(ResponseSchema {
                    name: "clarify_response".into(),
                    schema: json!({"type":"object","properties":{}}),
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // Schema tool call must NOT be executed as a real tool.
    assert!(
        !tool_executor_called,
        "schema tool must not be forwarded to executor"
    );

    // structured_response must contain the raw tool input.
    let sr = output
        .structured_response
        .expect("structured_response should be Some");
    assert_eq!(sr["question_type"], "Trend");
    assert_eq!(sr["metrics"][0], "revenue");

    // text should be JSON-serialised form of the input.
    let reparsed: Value = serde_json::from_str(&output.text).expect("text must be valid JSON");
    assert_eq!(reparsed["question_type"], "Trend");
}

// ── Test 10: Structured output — OpenAI response_format (text) path ───────
//
// When `response_schema` is set and the model returns plain JSON text
// (OpenAI response_format path — no tool calls), `run_with_tools` must
// parse the text and populate `structured_response`.

#[tokio::test]
async fn structured_response_openai_text_path_populates_structured_response() {
    let provider = MockProvider::new(vec![vec![
        Ok(Chunk::Text(
            "{\"sql\":\"SELECT COUNT(*) FROM orders\"}".into(),
        )),
        Ok(Chunk::Done(Usage {
            input_tokens: 10,
            output_tokens: 15,
            ..Default::default()
        })),
    ]]);

    let client = LlmClient::with_provider(provider);

    let output = client
        .run_with_tools::<(), _>(
            "system",
            "user",
            &[],
            |_, _| Box::pin(async { Ok(Box::new(Value::Null) as Box<dyn ToolOutput>) }),
            &None,
            ToolLoopConfig {
                state: "solving".into(),
                response_schema: Some(ResponseSchema {
                    name: "solve_response".into(),
                    schema: json!({"type":"object","properties":{"sql":{"type":"string"}},"required":["sql"]}),
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let sr = output
        .structured_response
        .expect("structured_response should be Some on JSON text path");
    assert_eq!(sr["sql"], "SELECT COUNT(*) FROM orders");
    assert_eq!(output.text, "{\"sql\":\"SELECT COUNT(*) FROM orders\"}");
}

// ── Test 11: Real tools run first, then schema tool terminates the loop ────
//
// The model first calls a real tool, then calls the schema response tool.
// The real tool must be executed; the schema tool must be intercepted.

#[tokio::test]
async fn real_tools_run_then_schema_tool_terminates_loop() {
    let provider = MockProvider::new(vec![
        // Round 1: real tool call
        vec![
            Ok(Chunk::ToolCall(ToolCallChunk {
                id: "tc_real".into(),
                name: "get_column_range".into(),
                input: json!({"dimension": "month"}),
                provider_data: None,
            })),
            Ok(Chunk::Done(Usage::default())),
        ],
        // Round 2: schema response tool call (signals end of loop)
        vec![
            Ok(Chunk::ToolCall(ToolCallChunk {
                id: "tc_schema".into(),
                name: "clarify_response".into(),
                input: json!({"question_type": "Breakdown", "metrics": ["orders"], "dimensions": ["region"], "filters": []}),
                provider_data: None,
            })),
            Ok(Chunk::Done(Usage {
                input_tokens: 30,
                output_tokens: 20,
                ..Default::default()
            })),
        ],
    ]);

    let client = LlmClient::with_provider(provider);
    let mut real_tool_called = false;

    let output = client
        .run_with_tools::<(), _>(
            "system",
            "user",
            &[],
            |name: String, _| {
                let result = if name == "get_column_range" {
                    real_tool_called = true;
                    json!({"min": "2024-01", "max": "2024-12"})
                } else {
                    panic!("unexpected tool call: {name}");
                };
                Box::pin(async move { Ok(Box::new(result) as Box<dyn ToolOutput>) })
            },
            &None,
            ToolLoopConfig {
                max_tool_rounds: 5,
                state: "clarifying".into(),
                thinking: ThinkingConfig::Disabled,
                response_schema: Some(ResponseSchema {
                    name: "clarify_response".into(),
                    schema: json!({"type":"object","properties":{}}),
                }),
                max_tokens_override: None,
                sub_spec_index: None,
                system_date_hint: None,
            },
        )
        .await
        .unwrap();

    assert!(real_tool_called, "the real tool must have been executed");

    let sr = output
        .structured_response
        .expect("structured_response must be Some");
    assert_eq!(sr["question_type"], "Breakdown");
    assert_eq!(sr["dimensions"][0], "region");
}

// ── Test 12: No schema — structured_response is None ─────────────────────

#[tokio::test]
async fn no_response_schema_gives_none_structured_response() {
    let provider = MockProvider::new(vec![vec![
        Ok(Chunk::Text("plain text response".into())),
        Ok(Chunk::Done(Usage::default())),
    ]]);

    let client = LlmClient::with_provider(provider);

    let output = client
        .run_with_tools::<(), _>(
            "system",
            "user",
            &[],
            |_, _| Box::pin(async { Ok(Box::new(Value::Null) as Box<dyn ToolOutput>) }),
            &None,
            ToolLoopConfig {
                state: "interpreting".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert!(
        output.structured_response.is_none(),
        "structured_response must be None when no schema is configured"
    );
    assert_eq!(output.text, "plain text response");
}

// ── Test 13: Anthropic render_chart tool_use / tool_result pairing ────────
//
// Reproduces the real-world Anthropic error:
//   "messages.1: `tool_use` ids were found without `tool_result` blocks
//    immediately after"
//
// The interpreting state uses `response_schema: None`, so the tool loop
// must construct a correct assistant(tool_use) → user(tool_result) pair.
// Unlike clarifying/solving which exit via the schema-tool intercept
// path, interpreting relies on the model returning plain text to
// terminate.

#[tokio::test]
async fn anthropic_render_chart_tool_use_result_pairing() {
    let captured = Arc::new(Mutex::new(Vec::new()));

    let provider = AnthropicMockProvider::new(
        vec![
            // Round 0: model returns text + render_chart tool call.
            vec![
                Ok(Chunk::Text(
                    "Let me create a visualization of your workout data.".into(),
                )),
                Ok(Chunk::ToolCall(ToolCallChunk {
                    id: "toolu_01C7NNbixDyhaRtZvWL36JmR".into(),
                    name: "render_chart".into(),
                    input: json!({
                        "chart_type": "bar",
                        "data": {
                            "columns": ["Day of Week", "Strength", "Climbing", "Cardio"],
                            "rows": [
                                ["Monday", "4", "4", "0"],
                                ["Tuesday", "3", "0", "1"],
                                ["Wednesday", "0", "0", "2"],
                                ["Thursday", "4", "0", "0"],
                                ["Friday", "0", "3", "1"]
                            ]
                        }
                    }),
                    provider_data: None,
                })),
                Ok(Chunk::Done(Usage {
                    input_tokens: 200,
                    output_tokens: 80,
                    ..Default::default()
                })),
            ],
            // Round 1: model returns final text answer (no tool calls).
            vec![
                Ok(Chunk::Text(
                    "Monday is your most active day with 8 total sessions.".into(),
                )),
                Ok(Chunk::Done(Usage {
                    input_tokens: 300,
                    output_tokens: 40,
                    ..Default::default()
                })),
            ],
        ],
        captured.clone(),
    );

    let client = LlmClient::with_provider(provider);

    let output = client
        .run_with_tools::<(), _>(
            "You are an analytics interpreter.",
            "What day do I work out most?",
            &[ToolDef {
                name: "render_chart",
                description: "Render a chart from data. Returns {chart_url}.",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "data": {"type": "object"},
                        "chart_type": {"type": "string"}
                    },
                    "required": ["data", "chart_type"]
                }),
                ..Default::default()
            }],
            |name: String, params| {
                assert_eq!(name, "render_chart");
                let ct = params["chart_type"]
                    .as_str()
                    .unwrap_or("unknown")
                    .to_string();
                Box::pin(async move {
                    Ok(Box::new(
                        json!({"chart_url": format!("https://charts.internal/render?type={ct}")}),
                    ) as Box<dyn ToolOutput>)
                })
            },
            &None,
            ToolLoopConfig {
                max_tool_rounds: 2,
                state: "interpreting".into(),
                thinking: ThinkingConfig::Disabled,
                response_schema: None,
                max_tokens_override: None,
                sub_spec_index: None,
                system_date_hint: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(
        output.text,
        "Let me create a visualization of your workout data.\nMonday is your most active day with 8 total sessions."
    );

    let rounds = captured.lock().unwrap();
    assert_eq!(rounds.len(), 2, "expected 2 API calls (rounds)");

    assert_eq!(rounds[0].len(), 1, "round 0 should have 1 message (user)");
    assert_eq!(rounds[0][0]["role"].as_str(), Some("user"));

    // Round 1: user + assistant(tool_use) + user(tool_result).
    assert_eq!(
        rounds[1].len(),
        3,
        "round 1 should have 3 messages [user, assistant, user(tool_result)]"
    );
    assert_eq!(rounds[1][0]["role"].as_str(), Some("user"));
    assert_eq!(rounds[1][1]["role"].as_str(), Some("assistant"));
    assert_eq!(rounds[1][2]["role"].as_str(), Some("user"));

    // Verify the assistant message contains the tool_use block.
    let assistant_content = rounds[1][1]["content"]
        .as_array()
        .expect("assistant content must be an array");
    let tool_use_blocks: Vec<&Value> = assistant_content
        .iter()
        .filter(|b| b["type"].as_str() == Some("tool_use"))
        .collect();
    assert_eq!(
        tool_use_blocks.len(),
        1,
        "assistant should have exactly 1 tool_use block"
    );
    assert_eq!(
        tool_use_blocks[0]["id"].as_str(),
        Some("toolu_01C7NNbixDyhaRtZvWL36JmR")
    );

    // Verify the tool_result message has the matching ID.
    let result_content = rounds[1][2]["content"]
        .as_array()
        .expect("tool_result content must be an array");
    let result_blocks: Vec<&Value> = result_content
        .iter()
        .filter(|b| b["type"].as_str() == Some("tool_result"))
        .collect();
    assert_eq!(
        result_blocks.len(),
        1,
        "user message should have exactly 1 tool_result block"
    );
    assert_eq!(
        result_blocks[0]["tool_use_id"].as_str(),
        Some("toolu_01C7NNbixDyhaRtZvWL36JmR"),
        "tool_result must reference the same id as the tool_use"
    );

    // Verify content block ordering: Anthropic requires text to precede
    // tool_use in the assistant message (matching model generation order).
    // If this fails, the API may reject with:
    //   "tool_use ids were found without tool_result blocks"
    let block_types: Vec<&str> = assistant_content
        .iter()
        .filter_map(|b| b["type"].as_str())
        .collect();
    assert_eq!(
        block_types,
        vec!["text", "tool_use"],
        "Content block ordering: text must precede tool_use in assistant message \
         to match model generation order. Actual: {block_types:?}"
    );

    // Full structural validation.
    validate_anthropic_messages(&rounds[1]).unwrap_or_else(|e| {
        for (i, msg) in rounds[1].iter().enumerate() {
            eprintln!(
                "messages[{i}] = {}",
                serde_json::to_string_pretty(msg).unwrap()
            );
        }
        panic!("Anthropic message validation failed: {e}");
    });
}

// ── Test 14: Anthropic render_chart with thinking blocks ──────────────────
//
// When thinking is enabled (e.g. via override), the assistant message
// contains thinking blocks interleaved with tool_use.  The tool_result
// pairing must still hold.

#[tokio::test]
async fn anthropic_render_chart_with_thinking_tool_use_result_pairing() {
    let captured = Arc::new(Mutex::new(Vec::new()));

    let think_blob = json!({
        "type": "thinking",
        "thinking": "The user wants a chart of workouts.",
        "signature": "sig_test_abc"
    });

    let provider = AnthropicMockProvider::new(
        vec![
            // Round 0: thinking + tool call (no text).
            vec![
                Ok(Chunk::ThinkingSummary(String::new())),
                Ok(Chunk::ThinkingSummary(
                    "The user wants a chart of workouts.".into(),
                )),
                Ok(Chunk::RawBlock(ContentBlock::Thinking {
                    provider_data: think_blob.clone(),
                })),
                Ok(Chunk::ToolCall(ToolCallChunk {
                    id: "toolu_think_test".into(),
                    name: "render_chart".into(),
                    input: json!({"chart_type": "bar", "data": {"columns": ["A"], "rows": [["1"]]}}),
                    provider_data: None,
                })),
                Ok(Chunk::Done(Usage::default())),
            ],
            // Round 1: final text.
            vec![
                Ok(Chunk::Text("Here is the analysis.".into())),
                Ok(Chunk::Done(Usage {
                    input_tokens: 50,
                    output_tokens: 10,
                    ..Default::default()
                })),
            ],
        ],
        captured.clone(),
    );

    let client = LlmClient::with_provider(provider);

    let output = client
        .run_with_tools::<(), _>(
            "system",
            "user",
            &[ToolDef {
                name: "render_chart",
                description: "Render a chart.",
                parameters: json!({"type": "object"}),
                ..Default::default()
            }],
            |_, _| {
                Box::pin(async {
                    Ok(
                        Box::new(json!({"chart_url": "https://charts.internal/render?type=bar"}))
                            as Box<dyn ToolOutput>,
                    )
                })
            },
            &None,
            ToolLoopConfig {
                max_tool_rounds: 2,
                state: "interpreting".into(),
                thinking: ThinkingConfig::Adaptive,
                response_schema: None,
                max_tokens_override: None,
                sub_spec_index: None,
                system_date_hint: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(output.text, "Here is the analysis.");

    let rounds = captured.lock().unwrap();
    assert_eq!(rounds.len(), 2);

    // Round 1 messages: user, assistant(thinking + tool_use), user(tool_result).
    assert_eq!(rounds[1].len(), 3);

    // The assistant message must contain both thinking and tool_use blocks.
    let assistant_content = rounds[1][1]["content"].as_array().unwrap();
    let block_types: Vec<&str> = assistant_content
        .iter()
        .filter_map(|b| b["type"].as_str())
        .collect();
    assert!(
        block_types.contains(&"thinking"),
        "assistant should include thinking block: {block_types:?}"
    );
    assert!(
        block_types.contains(&"tool_use"),
        "assistant should include tool_use block: {block_types:?}"
    );

    // Full structural validation — thinking blocks must not break pairing.
    validate_anthropic_messages(&rounds[1]).unwrap_or_else(|e| {
        for (i, msg) in rounds[1].iter().enumerate() {
            eprintln!(
                "messages[{i}] = {}",
                serde_json::to_string_pretty(msg).unwrap()
            );
        }
        panic!("Anthropic message validation failed: {e}");
    });
}

// ── Test 15: Anthropic two render_chart calls in one turn ─────────────────
//
// If the model issues two tool_use calls in a single assistant turn,
// both must have matching tool_result entries.

#[tokio::test]
async fn anthropic_two_tool_calls_both_get_tool_results() {
    let captured = Arc::new(Mutex::new(Vec::new()));

    let provider = AnthropicMockProvider::new(
        vec![
            // Round 0: two render_chart calls.
            vec![
                Ok(Chunk::ToolCall(ToolCallChunk {
                    id: "toolu_aaa".into(),
                    name: "render_chart".into(),
                    input: json!({"chart_type": "bar", "data": {"columns": ["A"], "rows": [["1"]]}}),
                    provider_data: None,
                })),
                Ok(Chunk::ToolCall(ToolCallChunk {
                    id: "toolu_bbb".into(),
                    name: "render_chart".into(),
                    input: json!({"chart_type": "line", "data": {"columns": ["B"], "rows": [["2"]]}}),
                    provider_data: None,
                })),
                Ok(Chunk::Done(Usage::default())),
            ],
            // Round 1: final text.
            vec![
                Ok(Chunk::Text("Two charts rendered.".into())),
                Ok(Chunk::Done(Usage::default())),
            ],
        ],
        captured.clone(),
    );

    let client = LlmClient::with_provider(provider);
    let mut calls = Vec::new();

    let output = client
        .run_with_tools::<(), _>(
            "system",
            "user",
            &[ToolDef {
                name: "render_chart",
                description: "Render a chart.",
                parameters: json!({"type": "object"}),
                ..Default::default()
            }],
            |name: String, _| {
                calls.push(name);
                Box::pin(async { Ok(Box::new(json!({"chart_url": "ok"})) as Box<dyn ToolOutput>) })
            },
            &None,
            ToolLoopConfig {
                max_tool_rounds: 3,
                state: "interpreting".into(),
                thinking: ThinkingConfig::Disabled,
                response_schema: None,
                max_tokens_override: None,
                sub_spec_index: None,
                system_date_hint: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(output.text, "Two charts rendered.");
    assert_eq!(calls, vec!["render_chart", "render_chart"]);

    let rounds = captured.lock().unwrap();

    // Round 1 messages must have tool_results for BOTH tool_use ids.
    let result_content = rounds[1][2]["content"].as_array().unwrap();
    let result_ids: Vec<&str> = result_content
        .iter()
        .filter_map(|b| b["tool_use_id"].as_str())
        .collect();
    assert!(
        result_ids.contains(&"toolu_aaa"),
        "missing tool_result for toolu_aaa: {result_ids:?}"
    );
    assert!(
        result_ids.contains(&"toolu_bbb"),
        "missing tool_result for toolu_bbb: {result_ids:?}"
    );

    validate_anthropic_messages(&rounds[1]).unwrap_or_else(|e| {
        for (i, msg) in rounds[1].iter().enumerate() {
            eprintln!(
                "messages[{i}] = {}",
                serde_json::to_string_pretty(msg).unwrap()
            );
        }
        panic!("Anthropic message validation failed: {e}");
    });
}

// ── Test 16: Anthropic multi-round tool calls ─────────────────────────────
//
// The model calls render_chart in round 0, gets the result, then calls
// it again in round 1 with different params.  Both rounds must have
// correct pairing.  The third-round final messages must satisfy the
// constraint for ALL previous tool_use/tool_result pairs.

#[tokio::test]
async fn anthropic_multi_round_tool_calls_all_paired() {
    let captured = Arc::new(Mutex::new(Vec::new()));

    let provider = AnthropicMockProvider::new(
        vec![
            // Round 0: first render_chart.
            vec![
                Ok(Chunk::ToolCall(ToolCallChunk {
                    id: "toolu_r0".into(),
                    name: "render_chart".into(),
                    input: json!({"chart_type": "bar", "data": {"columns": ["X"], "rows": [["1"]]}}),
                    provider_data: None,
                })),
                Ok(Chunk::Done(Usage::default())),
            ],
            // Round 1: second render_chart.
            vec![
                Ok(Chunk::Text("Let me also show a line chart.".into())),
                Ok(Chunk::ToolCall(ToolCallChunk {
                    id: "toolu_r1".into(),
                    name: "render_chart".into(),
                    input: json!({"chart_type": "line", "data": {"columns": ["X"], "rows": [["2"]]}}),
                    provider_data: None,
                })),
                Ok(Chunk::Done(Usage::default())),
            ],
            // Round 2: final text.
            vec![
                Ok(Chunk::Text("Analysis complete.".into())),
                Ok(Chunk::Done(Usage::default())),
            ],
        ],
        captured.clone(),
    );

    let client = LlmClient::with_provider(provider);

    let output = client
        .run_with_tools::<(), _>(
            "system",
            "user",
            &[ToolDef {
                name: "render_chart",
                description: "Render a chart.",
                parameters: json!({"type": "object"}),
                ..Default::default()
            }],
            |_, _| {
                Box::pin(async { Ok(Box::new(json!({"chart_url": "ok"})) as Box<dyn ToolOutput>) })
            },
            &None,
            ToolLoopConfig {
                max_tool_rounds: 5,
                state: "interpreting".into(),
                thinking: ThinkingConfig::Disabled,
                response_schema: None,
                max_tokens_override: None,
                sub_spec_index: None,
                system_date_hint: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(
        output.text,
        "Let me also show a line chart.\nAnalysis complete."
    );

    let rounds = captured.lock().unwrap();
    assert_eq!(rounds.len(), 3, "expected 3 API calls");

    // Validate EVERY round's messages (the full conversation grows each
    // round, so the final round has messaging from all previous rounds).
    for (round_idx, round_msgs) in rounds.iter().enumerate() {
        validate_anthropic_messages(round_msgs).unwrap_or_else(|e| {
            eprintln!("=== Round {round_idx} messages ===");
            for (i, msg) in round_msgs.iter().enumerate() {
                eprintln!(
                    "  messages[{i}] = {}",
                    serde_json::to_string_pretty(msg).unwrap()
                );
            }
            panic!("Anthropic message validation failed on round {round_idx}: {e}");
        });
    }
}

// ── Test: Thinking-only + MaxTokens triggers retry with doubled budget ────

#[tokio::test]
async fn thinking_only_max_tokens_retries_with_doubled_budget() {
    let provider = MockProvider::new(vec![
        // Round 1: thinking-only, truncated
        vec![
            Ok(Chunk::ThinkingSummary("Reasoning…".into())),
            Ok(Chunk::RawBlock(ContentBlock::Thinking {
                provider_data: json!({"type":"thinking","thinking":"...","signature":"sig"}),
            })),
            Ok(Chunk::Done(Usage {
                input_tokens: 100,
                output_tokens: 16384,
                stop_reason: StopReason::MaxTokens,
                ..Default::default()
            })),
        ],
        // Round 2: thinking + text, successful
        vec![
            Ok(Chunk::ThinkingSummary("More reasoning.".into())),
            Ok(Chunk::RawBlock(ContentBlock::Thinking {
                provider_data: json!({"type":"thinking","thinking":"...","signature":"sig2"}),
            })),
            Ok(Chunk::Text("Final answer".into())),
            Ok(Chunk::Done(Usage {
                input_tokens: 200,
                output_tokens: 500,
                stop_reason: StopReason::EndTurn,
                ..Default::default()
            })),
        ],
    ]);

    let client = LlmClient::with_provider(provider);
    let output = client
        .run_with_tools::<(), _>(
            "system",
            "user",
            &[],
            |_, _| Box::pin(async { Ok(Box::new(Value::Null) as Box<dyn ToolOutput>) }),
            &None,
            ToolLoopConfig {
                max_tool_rounds: 5,
                state: "interpreting".into(),
                thinking: ThinkingConfig::Disabled,
                response_schema: None,
                max_tokens_override: None,
                sub_spec_index: None,
                system_date_hint: None,
            },
        )
        .await
        .expect("should succeed after retry");

    assert_eq!(output.text, "Final answer");
    assert!(output.thinking_summary.is_some());
}

// ── Test: Thinking-only + MaxTokens exhausts retries → EmptyResponse ──────

#[tokio::test]
async fn thinking_only_max_tokens_exhausted_returns_error() {
    let provider = MockProvider::new(vec![
        vec![
            Ok(Chunk::ThinkingSummary("Thinking…".into())),
            Ok(Chunk::RawBlock(ContentBlock::Thinking {
                provider_data: json!({"type":"thinking","thinking":"...","signature":"sig"}),
            })),
            Ok(Chunk::Done(Usage {
                input_tokens: 100,
                output_tokens: 16384,
                stop_reason: StopReason::MaxTokens,
                ..Default::default()
            })),
        ],
        vec![
            Ok(Chunk::ThinkingSummary("Still thinking…".into())),
            Ok(Chunk::RawBlock(ContentBlock::Thinking {
                provider_data: json!({"type":"thinking","thinking":"...","signature":"sig2"}),
            })),
            Ok(Chunk::Done(Usage {
                input_tokens: 200,
                output_tokens: 32768,
                stop_reason: StopReason::MaxTokens,
                ..Default::default()
            })),
        ],
    ]);

    let client = LlmClient::with_provider(provider);
    let result = client
        .run_with_tools::<(), _>(
            "system",
            "user",
            &[],
            |_, _| Box::pin(async { Ok(Box::new(Value::Null) as Box<dyn ToolOutput>) }),
            &None,
            ToolLoopConfig {
                max_tool_rounds: 1,
                state: "interpreting".into(),
                thinking: ThinkingConfig::Disabled,
                response_schema: None,
                max_tokens_override: None,
                sub_spec_index: None,
                system_date_hint: None,
            },
        )
        .await;

    match result {
        Err(LlmError::EmptyResponse { reason }) => {
            assert!(
                reason.contains("max_tokens"),
                "expected max_tokens in reason, got: {reason}"
            );
        }
        other => panic!("expected EmptyResponse error, got: {other:?}"),
    }
}

// ── ask_user suspension / resume context-loss tests ──────────────────────

/// Verify the basic suspension path: when a tool executor returns
/// `ToolError::Suspended`, `run_with_tools` must propagate it as
/// `LlmError::Suspended` with the original prompt and suggestions.
#[tokio::test]
async fn ask_user_suspension_propagates_error() {
    let provider = MockProvider::new(vec![
        // Round 1: LLM immediately calls ask_user
        vec![
            Ok(Chunk::ToolCall(ToolCallChunk {
                id: "ask1".into(),
                name: "ask_user".into(),
                input: json!({"prompt": "Which metric?", "suggestions": ["Revenue", "Users"]}),
                provider_data: None,
            })),
            Ok(Chunk::Done(Usage {
                input_tokens: 10,
                output_tokens: 5,
                ..Default::default()
            })),
        ],
    ]);
    let client = LlmClient::with_provider(provider);

    let result = client
        .run_with_tools::<(), _>(
            "system",
            "What is the revenue?",
            &[],
            |name, input| {
                Box::pin(async move {
                    if name == "ask_user" {
                        let prompt = input["prompt"].as_str().unwrap_or("").to_string();
                        let suggestions: Vec<String> = input["suggestions"]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(|v| v.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default();
                        Err(agentic_core::tools::ToolError::Suspended {
                            prompt,
                            suggestions,
                        })
                    } else {
                        Ok(Box::new(json!({"ok": true})) as Box<dyn ToolOutput>)
                    }
                })
            },
            &None,
            ToolLoopConfig {
                max_tool_rounds: 5,
                state: "specifying".into(),
                thinking: ThinkingConfig::Disabled,
                response_schema: None,
                max_tokens_override: None,
                sub_spec_index: None,
                system_date_hint: None,
            },
        )
        .await;

    match result {
        Err(LlmError::Suspended {
            prompt,
            suggestions,
            ..
        }) => {
            assert_eq!(prompt, "Which metric?");
            assert_eq!(suggestions, vec!["Revenue", "Users"]);
        }
        other => panic!("expected LlmError::Suspended, got: {other:?}"),
    }
}

/// Regression test for the context-loss bug:
///
/// When the LLM runs multiple tool rounds **before** calling `ask_user`,
/// `build_resume_messages` must include those prior rounds so the LLM
/// has its full tool-call history on resume.
///
/// Scenario:
///   Round 1 → LLM calls `dry_run`, gets result.
///   Round 2 → LLM calls `ask_user` → suspends.
///   Resume  → `build_resume_messages` is called.
///
/// Expected: resume messages = [user, asst(dry_run), result(dry_run),
///                               asst(ask_user), result(answer)] — 5 messages.
/// Actual (bug): only [user, asst(ask_user), result(answer)] — 3 messages.
/// The prior `dry_run` exchange is silently dropped.
#[tokio::test]
async fn ask_user_suspension_after_tool_rounds_preserves_prior_context() {
    let captured = Arc::new(Mutex::new(Vec::<Vec<Value>>::new()));
    let provider = AnthropicMockProvider::new(
        vec![
            // Round 1: LLM calls a real tool
            vec![
                Ok(Chunk::ToolCall(ToolCallChunk {
                    id: "tc1".into(),
                    name: "dry_run".into(),
                    input: json!({"sql": "SELECT revenue FROM sales"}),
                    provider_data: None,
                })),
                Ok(Chunk::Done(Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                    ..Default::default()
                })),
            ],
            // Round 2: LLM calls ask_user
            vec![
                Ok(Chunk::ToolCall(ToolCallChunk {
                    id: "ask1".into(),
                    name: "ask_user".into(),
                    input: json!({"prompt": "Which date range?", "suggestions": ["Last week", "Last month"]}),
                    provider_data: None,
                })),
                Ok(Chunk::Done(Usage {
                    input_tokens: 20,
                    output_tokens: 8,
                    ..Default::default()
                })),
            ],
        ],
        Arc::clone(&captured),
    );
    let client = LlmClient::with_provider(provider);

    let result = client
        .run_with_tools::<(), _>(
            "system",
            "What is the revenue?",
            &[],
            |name, input| {
                Box::pin(async move {
                    if name == "ask_user" {
                        let prompt = input["prompt"].as_str().unwrap_or("").to_string();
                        let suggestions: Vec<String> = input["suggestions"]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(|v| v.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default();
                        Err(agentic_core::tools::ToolError::Suspended {
                            prompt,
                            suggestions,
                        })
                    } else {
                        // dry_run succeeds
                        Ok(
                            Box::new(json!({"columns": ["revenue"], "rows": [["1000"]]}))
                                as Box<dyn ToolOutput>,
                        )
                    }
                })
            },
            &None,
            ToolLoopConfig {
                max_tool_rounds: 5,
                state: "specifying".into(),
                thinking: ThinkingConfig::Disabled,
                response_schema: None,
                max_tokens_override: None,
                sub_spec_index: None,
                system_date_hint: None,
            },
        )
        .await;

    let (prompt, suggestions, prior_messages) = match result {
        Err(LlmError::Suspended {
            prompt,
            suggestions,
            prior_messages,
        }) => (prompt, suggestions, prior_messages),
        other => panic!("expected LlmError::Suspended, got: {other:?}"),
    };
    assert_eq!(prompt, "Which date range?");

    // Build resume messages as the orchestrator would, passing back the
    // prior tool-round context returned with the suspension.
    let resume_msgs =
        client.build_resume_messages(&prior_messages, &prompt, &suggestions, "Last month");

    // With the fix, resume_msgs must have 5 messages:
    //   [user, asst(dry_run), result(dry_run), asst(ask_user), result(answer)]
    assert_eq!(
        resume_msgs.len(),
        5,
        "resume messages must include prior tool rounds; \
         got {} messages but expected 5 (user + dry_run round + ask_user round).",
        resume_msgs.len(),
    );
}

/// Complementary check: when `ask_user` is the **first and only** tool call
/// (no prior rounds), `build_resume_messages` should produce exactly 3
/// messages and the round-trip should be structurally valid.
#[tokio::test]
async fn ask_user_as_first_tool_call_resume_has_three_messages() {
    let captured = Arc::new(Mutex::new(Vec::<Vec<Value>>::new()));
    let provider = AnthropicMockProvider::new(
        vec![
            // Only round: LLM immediately calls ask_user
            vec![
                Ok(Chunk::ToolCall(ToolCallChunk {
                    id: "ask1".into(),
                    name: "ask_user".into(),
                    input: json!({"prompt": "Which metric?", "suggestions": ["Revenue", "Users"]}),
                    provider_data: None,
                })),
                Ok(Chunk::Done(Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                    ..Default::default()
                })),
            ],
        ],
        Arc::clone(&captured),
    );
    let client = LlmClient::with_provider(provider);

    let result = client
        .run_with_tools::<(), _>(
            "system",
            "What is the revenue?",
            &[],
            |name, input| {
                Box::pin(async move {
                    if name == "ask_user" {
                        let prompt = input["prompt"].as_str().unwrap_or("").to_string();
                        let suggestions: Vec<String> = input["suggestions"]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(|v| v.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default();
                        Err(agentic_core::tools::ToolError::Suspended {
                            prompt,
                            suggestions,
                        })
                    } else {
                        Ok(Box::new(json!({"ok": true})) as Box<dyn ToolOutput>)
                    }
                })
            },
            &None,
            ToolLoopConfig {
                max_tool_rounds: 5,
                state: "specifying".into(),
                thinking: ThinkingConfig::Disabled,
                response_schema: None,
                max_tokens_override: None,
                sub_spec_index: None,
                system_date_hint: None,
            },
        )
        .await;

    let (prompt, suggestions, prior_messages) = match result {
        Err(LlmError::Suspended {
            prompt,
            suggestions,
            prior_messages,
        }) => (prompt, suggestions, prior_messages),
        other => panic!("expected LlmError::Suspended, got: {other:?}"),
    };

    let resume_msgs =
        client.build_resume_messages(&prior_messages, &prompt, &suggestions, "Revenue");

    // When ask_user is the very first tool call, prior_messages = [user, asst(ask_user)],
    // so resume = [user, asst(ask_user), tool_result(answer)] = 3 messages.
    assert_eq!(resume_msgs.len(), 3);
    validate_anthropic_messages(&resume_msgs)
        .expect("resume messages should satisfy Anthropic tool-use constraints");
}

/// Regression guard for the pre-ground ambiguity resume bug.
///
/// When triage detects an ambiguous question it suspends **before** calling
/// the LLM for the ground phase, storing `stage_data: {}` (no
/// `conversation_history`).  Before the fix, `ground_impl` called
/// `build_resume_messages(&[], ...)` on resume, which produced a lone
/// `tool_result` block referencing the hardcoded fallback id `"ask_user_0"`.
/// Because no `tool_use` with that id exists in any prior assistant message,
/// Anthropic rejected it with:
///
///   "unexpected `tool_use_id` found in `tool_result` blocks: ask_user_0.
///    Each `tool_result` block must have a corresponding `tool_use` block
///    in the previous message."
///
/// This test pins the broken behaviour so we know the empty-prior path
/// is structurally invalid and must not be used.  The fix instead passes the
/// user's answer through the user prompt (`InitialMessages::User`) when
/// `conversation_history` is absent from `stage_data`.
#[test]
fn build_resume_messages_with_empty_prior_fails_anthropic_validation() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let client =
        LlmClient::with_provider(AnthropicMockProvider::new(vec![], Arc::clone(&captured)));

    // Simulate what the old code did: call build_resume_messages with the
    // empty prior list that comes from stage_data["conversation_history"]
    // when the suspension was a pre-ground ambiguity (stage_data = {}).
    let msgs = client.build_resume_messages(&[], "Which metric?", &[], "Revenue");

    // The result contains a user message with a tool_result block whose
    // tool_use_id is "ask_user_0" (the fallback), but no assistant message
    // with a matching tool_use block precedes it.  Anthropic rejects this.
    assert!(
        validate_anthropic_messages(&msgs).is_err(),
        "empty prior_messages must produce a dangling tool_result that fails \
         Anthropic validation — this is exactly the scenario `ground_impl` must \
         not trigger on the pre-ground ambiguity resume path.\n\
         messages = {msgs:#?}"
    );
}

/// Validates that `build_resume_messages` with a well-formed prior
/// (containing an assistant message with a `tool_use` block for `ask_user`)
/// produces messages that pass Anthropic validation.
///
/// This is the positive counterpart to
/// `build_resume_messages_with_empty_prior_fails_anthropic_validation`.
#[test]
fn build_resume_messages_with_valid_prior_passes_anthropic_validation() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let client =
        LlmClient::with_provider(AnthropicMockProvider::new(vec![], Arc::clone(&captured)));

    // Simulate a prior conversation where the LLM called ask_user:
    //   [user(question), assistant(ask_user tool_use)]
    let prior = vec![
        json!({
            "role": "user",
            "content": "What is the revenue?"
        }),
        json!({
            "role": "assistant",
            "content": [
                {
                    "type": "tool_use",
                    "id": "toolu_ask123",
                    "name": "ask_user",
                    "input": {
                        "prompt": "Which metric?",
                        "suggestions": ["Revenue", "Users"]
                    }
                }
            ]
        }),
    ];

    let msgs =
        client.build_resume_messages(&prior, "Which metric?", &["Revenue".into()], "Revenue");

    // Should produce 3 messages: [user, asst(ask_user), user(tool_result)]
    assert_eq!(msgs.len(), 3, "expected 3 messages, got {}", msgs.len());
    validate_anthropic_messages(&msgs).expect(
        "build_resume_messages with a valid prior should produce messages that \
         satisfy Anthropic tool-use constraints",
    );
}

/// Verifies that `find_suspended_tool_id` returns `None` for an empty message list,
/// confirming the caller MUST handle the empty-prior case separately rather
/// than relying on the `"ask_user_0"` fallback in `build_resume_messages`.
#[test]
fn find_ask_user_id_returns_none_for_empty_messages() {
    use super::client::find_suspended_tool_id;
    assert!(
        find_suspended_tool_id(&[]).is_none(),
        "find_suspended_tool_id should return None for empty messages"
    );
}

// ── OpenAI Responses API mock provider ────────────────────────────────────

/// Mock provider that serialises messages in the **OpenAI Responses API**
/// format (flat items with `type: "function_call"` / `"function_call_output"`).
/// Used to test that `build_resume_messages` and `find_suspended_tool_id` correctly
/// handle the Responses API message shape.
struct OpenAiMockProvider {
    rounds: Mutex<VecDeque<Vec<Result<Chunk, LlmError>>>>,
    captured: Arc<Mutex<Vec<Vec<Value>>>>,
}

impl OpenAiMockProvider {
    fn new(
        rounds: Vec<Vec<Result<Chunk, LlmError>>>,
        captured: Arc<Mutex<Vec<Vec<Value>>>>,
    ) -> Self {
        Self {
            rounds: Mutex::new(rounds.into()),
            captured,
        }
    }
}

#[async_trait::async_trait]
impl LlmProvider for OpenAiMockProvider {
    async fn stream(
        &self,
        _system: &str,
        _system_date_suffix: &str,
        messages: &[Value],
        _tools: &[ToolDef],
        _thinking: &ThinkingConfig,
        _response_schema: Option<&ResponseSchema>,
        _max_tokens_override: Option<u32>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Chunk, LlmError>> + Send>>, LlmError> {
        self.captured.lock().unwrap().push(messages.to_vec());
        let chunks = self.rounds.lock().unwrap().pop_front().unwrap_or_default();
        Ok(Box::pin(tokio_stream::iter(chunks)))
    }

    /// OpenAI Responses API style: returns Value::Array of individual items.
    fn assistant_message(&self, blocks: &[ContentBlock]) -> Value {
        let mut items: Vec<Value> = Vec::new();
        let mut text_parts: Vec<Value> = Vec::new();

        let flush_text = |parts: &mut Vec<Value>, out: &mut Vec<Value>| {
            if !parts.is_empty() {
                out.push(json!({
                    "type": "message",
                    "role": "assistant",
                    "content": parts.drain(..).collect::<Vec<_>>()
                }));
            }
        };

        for block in blocks {
            match block {
                ContentBlock::ServerTool { .. } => {}
                ContentBlock::Text { text } => {
                    text_parts.push(json!({"type": "output_text", "text": text}));
                }
                ContentBlock::ToolUse {
                    id, name, input, ..
                } => {
                    flush_text(&mut text_parts, &mut items);
                    items.push(json!({
                        "type": "function_call",
                        "call_id": id,
                        "name": name,
                        "arguments": input.to_string()
                    }));
                }
                ContentBlock::Thinking { provider_data }
                | ContentBlock::RedactedThinking { provider_data } => {
                    flush_text(&mut text_parts, &mut items);
                    items.push(provider_data.clone());
                }
            }
        }
        flush_text(&mut text_parts, &mut items);
        Value::Array(items)
    }

    /// OpenAI Responses API style: one `function_call_output` per result.
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
        "mock-openai-responses"
    }
}

/// Validates OpenAI Responses API constraint: every `function_call` item
/// must be followed (eventually) by a `function_call_output` with the same
/// `call_id`.  Items may be nested inside arrays (assistant turns).
fn validate_openai_messages(messages: &[Value]) -> Result<(), String> {
    // Flatten: arrays become individual items, objects stay as-is.
    let flat: Vec<&Value> = messages
        .iter()
        .flat_map(|m| {
            if let Some(arr) = m.as_array() {
                arr.iter().collect::<Vec<_>>()
            } else {
                vec![m]
            }
        })
        .collect();

    let call_ids: Vec<&str> = flat
        .iter()
        .filter(|v| v["type"].as_str() == Some("function_call"))
        .filter_map(|v| v["call_id"].as_str())
        .collect();

    let output_ids: Vec<&str> = flat
        .iter()
        .filter(|v| v["type"].as_str() == Some("function_call_output"))
        .filter_map(|v| v["call_id"].as_str())
        .collect();

    for cid in &call_ids {
        if !output_ids.contains(cid) {
            return Err(format!(
                "No tool output found for function call {cid}. \
                 call_ids={call_ids:?}, output_ids={output_ids:?}"
            ));
        }
    }
    Ok(())
}

// ── OpenAI resume message tests ──────────────────────────────────────────

/// When the LLM calls `ask_user` after a tool round using the OpenAI Responses
/// API, `build_resume_messages` must produce a `function_call_output` whose
/// `call_id` matches the `ask_user` `function_call` item.
#[tokio::test]
async fn openai_ask_user_resume_after_tool_round_has_matching_call_id() {
    let captured = Arc::new(Mutex::new(Vec::<Vec<Value>>::new()));
    let provider = OpenAiMockProvider::new(
        vec![
            // Round 1: LLM calls a real tool
            vec![
                Ok(Chunk::ToolCall(ToolCallChunk {
                    id: "call_abc123".into(),
                    name: "dry_run".into(),
                    input: json!({"sql": "SELECT 1"}),
                    provider_data: None,
                })),
                Ok(Chunk::Done(Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                    ..Default::default()
                })),
            ],
            // Round 2: LLM calls ask_user
            vec![
                Ok(Chunk::ToolCall(ToolCallChunk {
                    id: "call_KjhgxBDSLRZ9l673lOWMi20G".into(),
                    name: "ask_user".into(),
                    input: json!({"prompt": "Which date range?", "suggestions": ["Last week", "Last month"]}),
                    provider_data: None,
                })),
                Ok(Chunk::Done(Usage {
                    input_tokens: 20,
                    output_tokens: 8,
                    ..Default::default()
                })),
            ],
        ],
        Arc::clone(&captured),
    );
    let client = LlmClient::with_provider(provider);

    let result = client
        .run_with_tools::<(), _>(
            "system",
            "What is the revenue?",
            &[],
            |name, input| {
                Box::pin(async move {
                    if name == "ask_user" {
                        let prompt = input["prompt"].as_str().unwrap_or("").to_string();
                        let suggestions: Vec<String> = input["suggestions"]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(|v| v.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default();
                        Err(agentic_core::tools::ToolError::Suspended {
                            prompt,
                            suggestions,
                        })
                    } else {
                        Ok(
                            Box::new(json!({"columns": ["revenue"], "rows": [["1000"]]}))
                                as Box<dyn ToolOutput>,
                        )
                    }
                })
            },
            &None,
            ToolLoopConfig {
                max_tool_rounds: 5,
                state: "ground".into(),
                thinking: ThinkingConfig::Disabled,
                response_schema: None,
                max_tokens_override: None,
                sub_spec_index: None,
                system_date_hint: None,
            },
        )
        .await;

    let (prompt, suggestions, prior_messages) = match result {
        Err(LlmError::Suspended {
            prompt,
            suggestions,
            prior_messages,
        }) => (prompt, suggestions, prior_messages),
        other => panic!("expected LlmError::Suspended, got: {other:?}"),
    };
    assert_eq!(prompt, "Which date range?");

    let resume_msgs =
        client.build_resume_messages(&prior_messages, &prompt, &suggestions, "Last month");

    // The last message must be a function_call_output with the correct call_id.
    let last = resume_msgs.last().unwrap();
    assert_eq!(
        last["type"].as_str(),
        Some("function_call_output"),
        "last message should be function_call_output, got: {last}"
    );
    assert_eq!(
        last["call_id"].as_str(),
        Some("call_KjhgxBDSLRZ9l673lOWMi20G"),
        "call_id must match the ask_user function_call, got: {last}"
    );
    assert_eq!(last["output"].as_str(), Some("Last month"));

    // Full structural validation: every function_call has a matching output.
    validate_openai_messages(&resume_msgs)
        .expect("resume messages should satisfy OpenAI function_call/output pairing");
}

/// When `ask_user` is the **first** tool call (no prior rounds) with the
/// OpenAI Responses API, `build_resume_messages` should still extract the
/// correct `call_id` from the array-wrapped assistant turn.
#[tokio::test]
async fn openai_ask_user_as_first_tool_call_has_matching_call_id() {
    let captured = Arc::new(Mutex::new(Vec::<Vec<Value>>::new()));
    let provider = OpenAiMockProvider::new(
        vec![vec![
            Ok(Chunk::ToolCall(ToolCallChunk {
                id: "call_first_ask".into(),
                name: "ask_user".into(),
                input: json!({"prompt": "Which metric?", "suggestions": ["Revenue", "Users"]}),
                provider_data: None,
            })),
            Ok(Chunk::Done(Usage {
                input_tokens: 10,
                output_tokens: 5,
                ..Default::default()
            })),
        ]],
        Arc::clone(&captured),
    );
    let client = LlmClient::with_provider(provider);

    let result = client
        .run_with_tools::<(), _>(
            "system",
            "Show me data",
            &[],
            |name, input| {
                Box::pin(async move {
                    if name == "ask_user" {
                        let prompt = input["prompt"].as_str().unwrap_or("").to_string();
                        let suggestions: Vec<String> = input["suggestions"]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(|v| v.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default();
                        Err(agentic_core::tools::ToolError::Suspended {
                            prompt,
                            suggestions,
                        })
                    } else {
                        Ok(Box::new(json!({"ok": true})) as Box<dyn ToolOutput>)
                    }
                })
            },
            &None,
            ToolLoopConfig {
                max_tool_rounds: 5,
                state: "ground".into(),
                thinking: ThinkingConfig::Disabled,
                response_schema: None,
                max_tokens_override: None,
                sub_spec_index: None,
                system_date_hint: None,
            },
        )
        .await;

    let (prompt, suggestions, prior_messages) = match result {
        Err(LlmError::Suspended {
            prompt,
            suggestions,
            prior_messages,
        }) => (prompt, suggestions, prior_messages),
        other => panic!("expected LlmError::Suspended, got: {other:?}"),
    };

    let resume_msgs =
        client.build_resume_messages(&prior_messages, &prompt, &suggestions, "Revenue");

    let last = resume_msgs.last().unwrap();
    assert_eq!(last["call_id"].as_str(), Some("call_first_ask"));
    assert_eq!(last["output"].as_str(), Some("Revenue"));

    validate_openai_messages(&resume_msgs)
        .expect("resume messages should satisfy OpenAI function_call/output pairing");
}

/// Unit test for `find_suspended_tool_id` with OpenAI Responses API format
/// (array-wrapped assistant message containing function_call items).
#[test]
fn find_suspended_tool_id_openai_responses_api_format() {
    use super::client::find_suspended_tool_id;

    let messages = vec![
        json!({"role": "user", "content": "test"}),
        // assistant_message() returns Value::Array for OpenAI Responses API
        json!([
            {"type": "function_call", "call_id": "call_xyz", "name": "dry_run", "arguments": "{}"},
        ]),
        json!({"type": "function_call_output", "call_id": "call_xyz", "output": "ok"}),
        json!([
            {"type": "function_call", "call_id": "call_ask123", "name": "ask_user", "arguments": "{}"},
        ]),
    ];
    assert_eq!(
        find_suspended_tool_id(&messages),
        Some("call_ask123".to_string()),
    );
}

/// Unit test for `find_suspended_tool_id` with OpenAI Chat Completions format.
#[test]
fn find_suspended_tool_id_openai_chat_completions_format() {
    use super::client::find_suspended_tool_id;

    let messages = vec![
        json!({"role": "user", "content": "test"}),
        json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [{
                "id": "call_chat_abc",
                "type": "function",
                "function": {"name": "ask_user", "arguments": "{}"}
            }]
        }),
    ];
    assert_eq!(
        find_suspended_tool_id(&messages),
        Some("call_chat_abc".to_string()),
    );
}

/// Unit test for `find_suspended_tool_id` with Anthropic format.
#[test]
fn find_suspended_tool_id_anthropic_format() {
    use super::client::find_suspended_tool_id;

    let messages = vec![
        json!({"role": "user", "content": "test"}),
        json!({
            "role": "assistant",
            "content": [
                {"type": "tool_use", "id": "toolu_abc", "name": "ask_user", "input": {}}
            ]
        }),
    ];
    assert_eq!(
        find_suspended_tool_id(&messages),
        Some("toolu_abc".to_string()),
    );
}

/// Unit test: `find_suspended_tool_id` finds `file_change` (not just `ask_user`).
/// This is the builder-pipeline regression — the suspended tool is `file_change`,
/// so the old name-based search returned None and fell back to "ask_user_0".
#[test]
fn find_suspended_tool_id_finds_file_change() {
    use super::client::find_suspended_tool_id;

    // Simulate: one search_files call (matched) followed by a file_change (unmatched).
    let messages = vec![
        json!({"role": "user", "content": "test"}),
        json!({
            "role": "assistant",
            "content": [
                {"type": "tool_use", "id": "toolu_search", "name": "search_files", "input": {}},
                {"type": "tool_use", "id": "toolu_propose", "name": "file_change", "input": {}}
            ]
        }),
        json!({
            "role": "user",
            "content": [
                {"type": "tool_result", "tool_use_id": "toolu_search", "content": "found 3 files"}
            ]
        }),
    ];
    assert_eq!(
        find_suspended_tool_id(&messages),
        Some("toolu_propose".to_string()),
        "must return file_change id, not None",
    );
}

/// Regression test: when the LLM calls multiple `file_change` tools in a
/// single batch (e.g. one per view file), the tool loop suspends on the FIRST
/// one.  `find_suspended_tool_id` must return the FIRST unmatched tool_use ID,
/// not the last — otherwise the resumed request has earlier tool_uses without
/// results and Anthropic rejects with
/// "tool_use ids were found without tool_result blocks immediately after".
#[test]
fn find_suspended_tool_id_batched_file_change_returns_first() {
    use super::client::find_suspended_tool_id;

    // All three file_change calls are in one assistant turn; none have
    // results yet (the first one suspended before any results were flushed).
    let messages = vec![
        json!({"role": "user", "content": "build the semantic model"}),
        json!({
            "role": "assistant",
            "content": [
                {"type": "tool_use", "id": "toolu_pc1", "name": "file_change", "input": {"file_path": "semantics/view1.view.yml"}},
                {"type": "tool_use", "id": "toolu_pc2", "name": "file_change", "input": {"file_path": "semantics/view2.view.yml"}},
                {"type": "tool_use", "id": "toolu_pc3", "name": "file_change", "input": {"file_path": "semantics/view3.view.yml"}}
            ]
        }),
    ];
    assert_eq!(
        find_suspended_tool_id(&messages),
        Some("toolu_pc1".to_string()),
        "must return the FIRST file_change id (the one that actually suspended)",
    );
}

/// Regression test: `build_resume_messages` must produce a `tool_result` for
/// every unmatched `tool_use` ID in the batch, not just the first.
/// Validates that the resulting messages satisfy Anthropic's constraint that
/// all `tool_use` IDs have matching `tool_result` blocks.
#[test]
fn build_resume_messages_batched_file_change_all_ids_resolved() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let client =
        LlmClient::with_provider(AnthropicMockProvider::new(vec![], Arc::clone(&captured)));

    let prior = vec![
        json!({"role": "user", "content": "build the semantic model"}),
        json!({
            "role": "assistant",
            "content": [
                {"type": "tool_use", "id": "toolu_pc1", "name": "file_change", "input": {"file_path": "v1.view.yml"}},
                {"type": "tool_use", "id": "toolu_pc2", "name": "file_change", "input": {"file_path": "v2.view.yml"}},
                {"type": "tool_use", "id": "toolu_pc3", "name": "file_change", "input": {"file_path": "v3.view.yml"}}
            ]
        }),
    ];

    let msgs = client.build_resume_messages(&prior, "", &[], "accepted");

    validate_anthropic_messages(&msgs)
        .expect("all three tool_use ids must have matching tool_results");

    let last = msgs.last().unwrap();
    let results: Vec<&str> = last["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| b["type"] == "tool_result")
        .map(|b| b["tool_use_id"].as_str().unwrap())
        .collect();
    assert_eq!(
        results,
        ["toolu_pc1", "toolu_pc2", "toolu_pc3"],
        "all three tool_use ids must appear in the tool_results"
    );
}

// ── Batched tool call + ask_user suspension test ─────────────────────────

/// Regression test: when the model calls multiple tools in one batch and
/// one of them is `ask_user`, the results of already-executed tools must
/// be flushed to `prior_messages` before the suspension early-return.
///
/// Without this fix, `prior_messages` contains:
///   [user, asst([dry_run, ask_user])]
/// but no `function_call_output` for `dry_run`, causing OpenAI to reject
/// the next API call with "No tool output found for function call".
#[tokio::test]
async fn openai_batched_tools_with_ask_user_flushes_prior_results() {
    let captured = Arc::new(Mutex::new(Vec::<Vec<Value>>::new()));
    let provider = OpenAiMockProvider::new(
        vec![
            // Single round: model calls both dry_run AND ask_user in one batch
            vec![
                Ok(Chunk::ToolCall(ToolCallChunk {
                    id: "call_dry".into(),
                    name: "dry_run".into(),
                    input: json!({"sql": "SELECT 1"}),
                    provider_data: None,
                })),
                Ok(Chunk::ToolCall(ToolCallChunk {
                    id: "call_ask".into(),
                    name: "ask_user".into(),
                    input: json!({"prompt": "Which date?", "suggestions": ["Today", "Yesterday"]}),
                    provider_data: None,
                })),
                Ok(Chunk::Done(Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                    ..Default::default()
                })),
            ],
        ],
        Arc::clone(&captured),
    );
    let client = LlmClient::with_provider(provider);

    let result = client
        .run_with_tools::<(), _>(
            "system",
            "What is the revenue?",
            &[],
            |name, input| {
                Box::pin(async move {
                    if name == "ask_user" {
                        let prompt = input["prompt"].as_str().unwrap_or("").to_string();
                        let suggestions: Vec<String> = input["suggestions"]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(|v| v.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default();
                        Err(agentic_core::tools::ToolError::Suspended {
                            prompt,
                            suggestions,
                        })
                    } else {
                        Ok(
                            Box::new(json!({"columns": ["revenue"], "rows": [["1000"]]}))
                                as Box<dyn ToolOutput>,
                        )
                    }
                })
            },
            &None,
            ToolLoopConfig {
                max_tool_rounds: 5,
                state: "ground".into(),
                thinking: ThinkingConfig::Disabled,
                response_schema: None,
                max_tokens_override: None,
                sub_spec_index: None,
                system_date_hint: None,
            },
        )
        .await;

    let (prompt, suggestions, prior_messages) = match result {
        Err(LlmError::Suspended {
            prompt,
            suggestions,
            prior_messages,
        }) => (prompt, suggestions, prior_messages),
        other => panic!("expected LlmError::Suspended, got: {other:?}"),
    };
    assert_eq!(prompt, "Which date?");

    // Before fix: prior_messages = [user, asst([dry_run, ask_user])]
    //   → no function_call_output for dry_run → OpenAI rejects
    //
    // After fix: prior_messages = [user, asst([dry_run, ask_user]), function_call_output(dry_run)]
    //   → dry_run has its output → build_resume_messages adds ask_user output → valid

    let resume_msgs = client.build_resume_messages(&prior_messages, &prompt, &suggestions, "Today");

    // Validate every function_call has a matching function_call_output.
    validate_openai_messages(&resume_msgs).expect(
        "resume messages must have matching function_call_output for ALL \
         function_calls, including those executed before ask_user in the batch",
    );

    // The last message should be the ask_user answer.
    let last = resume_msgs.last().unwrap();
    assert_eq!(last["call_id"].as_str(), Some("call_ask"));
    assert_eq!(last["output"].as_str(), Some("Today"));
}

/// Same bug but with Anthropic format — validates that already-executed tool
/// results are flushed before suspension.
#[tokio::test]
async fn anthropic_batched_tools_with_ask_user_flushes_prior_results() {
    let captured = Arc::new(Mutex::new(Vec::<Vec<Value>>::new()));
    let provider = AnthropicMockProvider::new(
        vec![
            // Single round: model calls both dry_run AND ask_user in one batch
            vec![
                Ok(Chunk::ToolCall(ToolCallChunk {
                    id: "tc_dry".into(),
                    name: "dry_run".into(),
                    input: json!({"sql": "SELECT 1"}),
                    provider_data: None,
                })),
                Ok(Chunk::ToolCall(ToolCallChunk {
                    id: "tc_ask".into(),
                    name: "ask_user".into(),
                    input: json!({"prompt": "Which date?", "suggestions": ["Today", "Yesterday"]}),
                    provider_data: None,
                })),
                Ok(Chunk::Done(Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                    ..Default::default()
                })),
            ],
        ],
        Arc::clone(&captured),
    );
    let client = LlmClient::with_provider(provider);

    let result = client
        .run_with_tools::<(), _>(
            "system",
            "What is the revenue?",
            &[],
            |name, input| {
                Box::pin(async move {
                    if name == "ask_user" {
                        let prompt = input["prompt"].as_str().unwrap_or("").to_string();
                        let suggestions: Vec<String> = input["suggestions"]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(|v| v.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default();
                        Err(agentic_core::tools::ToolError::Suspended {
                            prompt,
                            suggestions,
                        })
                    } else {
                        Ok(
                            Box::new(json!({"columns": ["revenue"], "rows": [["1000"]]}))
                                as Box<dyn ToolOutput>,
                        )
                    }
                })
            },
            &None,
            ToolLoopConfig {
                max_tool_rounds: 5,
                state: "ground".into(),
                thinking: ThinkingConfig::Disabled,
                response_schema: None,
                max_tokens_override: None,
                sub_spec_index: None,
                system_date_hint: None,
            },
        )
        .await;

    let (prompt, suggestions, prior_messages) = match result {
        Err(LlmError::Suspended {
            prompt,
            suggestions,
            prior_messages,
        }) => (prompt, suggestions, prior_messages),
        other => panic!("expected LlmError::Suspended, got: {other:?}"),
    };

    let resume_msgs = client.build_resume_messages(&prior_messages, &prompt, &suggestions, "Today");

    validate_anthropic_messages(&resume_msgs).expect(
        "resume messages must have matching tool_result for ALL tool_use blocks, \
         including those executed before ask_user in the batch",
    );
}

// ── MaxTokensReached / MaxToolRoundsReached suspension tests ─────────────

/// When the model emits text and then stops with `StopReason::MaxTokens`,
/// `run_with_tools` must return `LlmError::MaxTokensReached` with the
/// partial text and the budget that was exhausted.
#[tokio::test]
async fn text_truncated_by_max_tokens_returns_max_tokens_reached() {
    let provider = MockProvider::new(vec![vec![
        Ok(Chunk::Text("Partial answer".into())),
        Ok(Chunk::Done(Usage {
            input_tokens: 10,
            output_tokens: 4096,
            stop_reason: StopReason::MaxTokens,
            ..Default::default()
        })),
    ]]);

    let client = LlmClient::with_provider(provider);
    let result = client
        .run_with_tools::<(), _>(
            "system",
            "user",
            &[],
            |_, _| Box::pin(async { Ok(Box::new(Value::Null) as Box<dyn ToolOutput>) }),
            &None,
            ToolLoopConfig {
                max_tool_rounds: 5,
                state: "test".into(),
                thinking: ThinkingConfig::Disabled,
                response_schema: None,
                max_tokens_override: None,
                sub_spec_index: None,
                system_date_hint: None,
            },
        )
        .await;

    match result {
        Err(LlmError::MaxTokensReached {
            partial_text,
            current_max_tokens,
            ..
        }) => {
            assert_eq!(partial_text, "Partial answer");
            // DEFAULT_MAX_TOKENS is 4096 when no override is set
            assert_eq!(current_max_tokens, 4096);
        }
        other => panic!("expected MaxTokensReached, got: {other:?}"),
    }
}

/// `MaxTokensReached.prior_messages` must include an assistant message
/// containing the truncated text so the resume context is complete.
#[tokio::test]
async fn max_tokens_reached_prior_messages_contains_truncated_assistant_turn() {
    let captured = Arc::new(Mutex::new(Vec::<Vec<Value>>::new()));
    let provider = AnthropicMockProvider::new(
        vec![vec![
            Ok(Chunk::Text("Partial".into())),
            Ok(Chunk::Done(Usage {
                input_tokens: 5,
                output_tokens: 4096,
                stop_reason: StopReason::MaxTokens,
                ..Default::default()
            })),
        ]],
        Arc::clone(&captured),
    );

    let client = LlmClient::with_provider(provider);
    let result = client
        .run_with_tools::<(), _>(
            "system",
            "user question",
            &[],
            |_, _| Box::pin(async { Ok(Box::new(Value::Null) as Box<dyn ToolOutput>) }),
            &None,
            ToolLoopConfig {
                max_tool_rounds: 5,
                state: "test".into(),
                thinking: ThinkingConfig::Disabled,
                response_schema: None,
                max_tokens_override: None,
                sub_spec_index: None,
                system_date_hint: None,
            },
        )
        .await;

    let prior = match result {
        Err(LlmError::MaxTokensReached { prior_messages, .. }) => prior_messages,
        other => panic!("expected MaxTokensReached, got: {other:?}"),
    };

    // prior_messages = [user_message, assistant_message_with_partial_text]
    assert_eq!(
        prior.len(),
        2,
        "prior_messages should be [user, asst(partial)]"
    );
    let last = prior.last().unwrap();
    assert_eq!(last["role"].as_str(), Some("assistant"));
    let text_in_asst = last["content"]
        .as_array()
        .and_then(|arr| arr.iter().find(|b| b["type"].as_str() == Some("text")))
        .and_then(|b| b["text"].as_str())
        .unwrap_or("");
    assert_eq!(text_in_asst, "Partial");
}

/// `build_continue_messages` must append a user turn with "Please continue."
/// so the LLM resumes generation from where it was cut off.
#[test]
fn build_continue_messages_appends_please_continue_user_turn() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let _client =
        LlmClient::with_provider(AnthropicMockProvider::new(vec![], Arc::clone(&captured)));

    let prior = vec![
        json!({"role": "user", "content": "What is revenue?"}),
        json!({"role": "assistant", "content": [{"type": "text", "text": "Revenue is..."}]}),
    ];
    let continued = LlmClient::build_continue_messages(&prior);

    assert_eq!(continued.len(), 3, "should be prior + 1 user message");
    let last = continued.last().unwrap();
    assert_eq!(last["role"].as_str(), Some("user"));
    assert_eq!(last["content"].as_str(), Some("Please continue."));
}

/// When tool rounds are exhausted, `run_with_tools` must return
/// `LlmError::MaxToolRoundsReached` with the round count.
#[tokio::test]
async fn tool_rounds_exhausted_returns_max_tool_rounds_reached() {
    // 3 rounds of tool calls — max_tool_rounds = 2, so after round 2 we should
    // hit the limit on the *third* model response's tool-call batch.
    let provider = AnthropicMockProvider::new(
        vec![
            // Round 0
            vec![
                Ok(Chunk::ToolCall(ToolCallChunk {
                    id: "tc0".into(),
                    name: "noop".into(),
                    input: json!({}),
                    provider_data: None,
                })),
                Ok(Chunk::Done(Usage {
                    ..Default::default()
                })),
            ],
            // Round 1
            vec![
                Ok(Chunk::ToolCall(ToolCallChunk {
                    id: "tc1".into(),
                    name: "noop".into(),
                    input: json!({}),
                    provider_data: None,
                })),
                Ok(Chunk::Done(Usage {
                    ..Default::default()
                })),
            ],
            // Round 2 — hits limit (rounds >= max_tool_rounds=2)
            vec![
                Ok(Chunk::ToolCall(ToolCallChunk {
                    id: "tc2".into(),
                    name: "noop".into(),
                    input: json!({}),
                    provider_data: None,
                })),
                Ok(Chunk::Done(Usage {
                    ..Default::default()
                })),
            ],
        ],
        Arc::new(Mutex::new(vec![])),
    );

    let client = LlmClient::with_provider(provider);
    let result = client
        .run_with_tools::<(), _>(
            "system",
            "user",
            &[ToolDef {
                name: "noop",
                description: "does nothing",
                parameters: json!({"type": "object"}),
                ..Default::default()
            }],
            |_, _| Box::pin(async { Ok(Box::new(json!({})) as Box<dyn ToolOutput>) }),
            &None,
            ToolLoopConfig {
                max_tool_rounds: 2,
                state: "test".into(),
                thinking: ThinkingConfig::Disabled,
                response_schema: None,
                max_tokens_override: None,
                sub_spec_index: None,
                system_date_hint: None,
            },
        )
        .await;

    match result {
        Err(LlmError::MaxToolRoundsReached { rounds, .. }) => {
            assert_eq!(rounds, 2, "should report the configured limit");
        }
        other => panic!("expected MaxToolRoundsReached, got: {other:?}"),
    }
}

/// `MaxToolRoundsReached.prior_messages` is the history *before* the
/// current (unanswered) model tool request — the round that triggered the
/// limit should not be appended.
#[tokio::test]
async fn max_tool_rounds_prior_messages_excludes_current_tool_request() {
    let captured = Arc::new(Mutex::new(Vec::<Vec<Value>>::new()));
    let provider = AnthropicMockProvider::new(
        vec![
            // Round 0: tool call + result → messages grows to [user, asst, result]
            vec![
                Ok(Chunk::ToolCall(ToolCallChunk {
                    id: "tc0".into(),
                    name: "noop".into(),
                    input: json!({}),
                    provider_data: None,
                })),
                Ok(Chunk::Done(Usage {
                    ..Default::default()
                })),
            ],
            // Round 1 (rounds==1 >= max_tool_rounds==1): triggers the limit
            vec![
                Ok(Chunk::ToolCall(ToolCallChunk {
                    id: "tc1".into(),
                    name: "noop".into(),
                    input: json!({}),
                    provider_data: None,
                })),
                Ok(Chunk::Done(Usage {
                    ..Default::default()
                })),
            ],
        ],
        Arc::clone(&captured),
    );

    let client = LlmClient::with_provider(provider);
    let result = client
        .run_with_tools::<(), _>(
            "system",
            "user",
            &[ToolDef {
                name: "noop",
                description: "no-op",
                parameters: json!({"type": "object"}),
                ..Default::default()
            }],
            |_, _| Box::pin(async { Ok(Box::new(json!({})) as Box<dyn ToolOutput>) }),
            &None,
            ToolLoopConfig {
                max_tool_rounds: 1,
                state: "test".into(),
                thinking: ThinkingConfig::Disabled,
                response_schema: None,
                max_tokens_override: None,
                sub_spec_index: None,
                system_date_hint: None,
            },
        )
        .await;

    let prior = match result {
        Err(LlmError::MaxToolRoundsReached { prior_messages, .. }) => prior_messages,
        other => panic!("expected MaxToolRoundsReached, got: {other:?}"),
    };

    // [user, asst(tc0), result(tc0)] — 3 messages; the second model response
    // (tc1) was NOT appended because the limit fires before processing it.
    assert_eq!(
        prior.len(),
        3,
        "prior_messages should be [user, asst(tc0), result(tc0)]; got {prior:?}"
    );
}

/// Resume from `MaxTokensReached`: `build_continue_messages` produces messages
/// where the last user turn says "Please continue." and the Anthropic
/// tool-use constraints are satisfied throughout.
#[tokio::test]
async fn max_tokens_resume_via_build_continue_messages_is_valid() {
    let captured = Arc::new(Mutex::new(Vec::<Vec<Value>>::new()));
    let provider = AnthropicMockProvider::new(
        vec![vec![
            Ok(Chunk::Text("Truncated answer".into())),
            Ok(Chunk::Done(Usage {
                stop_reason: StopReason::MaxTokens,
                ..Default::default()
            })),
        ]],
        Arc::clone(&captured),
    );

    let client = LlmClient::with_provider(provider);
    let result = client
        .run_with_tools::<(), _>(
            "system",
            "original question",
            &[],
            |_, _| Box::pin(async { Ok(Box::new(Value::Null) as Box<dyn ToolOutput>) }),
            &None,
            ToolLoopConfig {
                max_tool_rounds: 5,
                state: "test".into(),
                thinking: ThinkingConfig::Disabled,
                response_schema: None,
                max_tokens_override: None,
                sub_spec_index: None,
                system_date_hint: None,
            },
        )
        .await;

    let (partial_text, prior) = match result {
        Err(LlmError::MaxTokensReached {
            partial_text,
            prior_messages,
            ..
        }) => (partial_text, prior_messages),
        other => panic!("expected MaxTokensReached, got: {other:?}"),
    };
    assert_eq!(partial_text, "Truncated answer");

    // Simulate the caller (solver) invoking build_continue_messages.
    let resume_msgs = LlmClient::build_continue_messages(&prior);

    // Structure: [user(original), asst(partial), user("Please continue.")]
    assert_eq!(resume_msgs.len(), 3);
    let last = resume_msgs.last().unwrap();
    assert_eq!(last["role"].as_str(), Some("user"));
    assert_eq!(last["content"].as_str(), Some("Please continue."));
}

// ── Anthropic prompt-caching wire format ──────────────────────────────────

/// `build_request_body` must mark the system content block with
/// `cache_control: ephemeral` so Anthropic caches the system prefix.
#[test]
fn build_request_body_marks_system_with_cache_control() {
    let provider = super::anthropic::AnthropicProvider::new("k", "claude-sonnet-4-6");
    let body = provider.build_request_body(
        "you are an assistant",
        "",
        &[],
        &[],
        &super::ThinkingConfig::Disabled,
        None,
        None,
    );
    let system = body["system"]
        .as_array()
        .expect("system field is a content-blocks array");
    assert_eq!(
        system.len(),
        1,
        "expected one system block when no date hint"
    );
    assert_eq!(system[0]["type"].as_str(), Some("text"));
    assert_eq!(system[0]["text"].as_str(), Some("you are an assistant"));
    assert_eq!(
        system[0]["cache_control"],
        json!({"type": "ephemeral"}),
        "system block must carry cache_control"
    );
}

/// When `system_date_suffix` is non-empty, the system field must contain
/// two blocks: the first cached, the second uncached.  This lets the
/// time-varying date string sit *after* the cache breakpoint.
#[test]
fn build_request_body_emits_two_system_blocks_when_date_suffix_set() {
    let provider = super::anthropic::AnthropicProvider::new("k", "claude-sonnet-4-6");
    let body = provider.build_request_body(
        "static instructions",
        "Today is 2026-04-25.",
        &[],
        &[],
        &super::ThinkingConfig::Disabled,
        None,
        None,
    );
    let system = body["system"].as_array().expect("system is array");
    assert_eq!(system.len(), 2, "expected static + date suffix blocks");
    assert_eq!(
        system[0]["text"].as_str(),
        Some("static instructions"),
        "first block is the static prefix"
    );
    assert_eq!(
        system[0]["cache_control"],
        json!({"type": "ephemeral"}),
        "static prefix must carry cache_control"
    );
    assert_eq!(
        system[1]["text"].as_str(),
        Some("Today is 2026-04-25."),
        "second block is the dynamic date suffix"
    );
    assert!(
        system[1].get("cache_control").is_none(),
        "date suffix must NOT carry cache_control — it would invalidate the prefix daily"
    );
}

/// Only the LAST tool gets `cache_control`.  Setting it on every tool
/// would consume one of Anthropic's four breakpoint slots per tool.
#[test]
fn build_request_body_marks_only_last_tool() {
    use agentic_core::tools::ToolDef;
    let tools = vec![
        ToolDef {
            name: "first".into(),
            description: "first tool".into(),
            parameters: json!({"type": "object", "properties": {}}),
            strict: false,
        },
        ToolDef {
            name: "second".into(),
            description: "second tool".into(),
            parameters: json!({"type": "object", "properties": {}}),
            strict: false,
        },
    ];
    let provider = super::anthropic::AnthropicProvider::new("k", "claude-sonnet-4-6");
    let body = provider.build_request_body(
        "system",
        "",
        &[],
        &tools,
        &super::ThinkingConfig::Disabled,
        None,
        None,
    );
    let tools_arr = body["tools"].as_array().expect("tools array present");
    assert_eq!(tools_arr.len(), 2);
    assert!(
        tools_arr[0].get("cache_control").is_none(),
        "first tool must NOT carry cache_control"
    );
    assert_eq!(
        tools_arr[1]["cache_control"],
        json!({"type": "ephemeral"}),
        "last tool must carry cache_control"
    );
}

/// When the system prompt is empty, no system block is emitted (and so no
/// orphan `cache_control` slot is consumed).
#[test]
fn build_request_body_omits_system_block_when_empty_system() {
    let provider = super::anthropic::AnthropicProvider::new("k", "claude-sonnet-4-6");
    let body = provider.build_request_body(
        "",
        "",
        &[],
        &[],
        &super::ThinkingConfig::Disabled,
        None,
        None,
    );
    let system = body["system"].as_array().expect("system is array");
    assert!(system.is_empty(), "no system block when prompt is empty");
}

/// Edge case: empty static system but a non-empty date suffix.  The single
/// emitted block is the date hint and must NOT carry `cache_control` — a
/// daily-rotating cache breakpoint would burn one of Anthropic's four slots
/// for nothing.
#[test]
fn build_request_body_emits_only_uncached_date_block_when_static_empty() {
    let provider = super::anthropic::AnthropicProvider::new("k", "claude-sonnet-4-6");
    let body = provider.build_request_body(
        "",
        "Today is 2026-04-25.",
        &[],
        &[],
        &super::ThinkingConfig::Disabled,
        None,
        None,
    );
    let system = body["system"].as_array().expect("system is array");
    assert_eq!(system.len(), 1, "expected just the date suffix block");
    assert_eq!(system[0]["text"].as_str(), Some("Today is 2026-04-25."));
    assert!(
        system[0].get("cache_control").is_none(),
        "date-only block must not carry cache_control"
    );
}

/// `Usage` cache fields must round-trip through `CoreEvent::LlmEnd` so
/// callers (frontend, tracing, DB) can compute hit ratios.  Uses the
/// existing mock-provider plumbing instead of a real HTTP server.
#[tokio::test]
async fn cache_tokens_propagate_from_provider_to_llm_end_event() {
    let provider = MockProvider::new(vec![vec![
        Ok(Chunk::Text(String::new())),
        Ok(Chunk::Text("hello".into())),
        Ok(Chunk::Done(Usage {
            input_tokens: 100,
            output_tokens: 20,
            cache_creation_input_tokens: 1234,
            cache_read_input_tokens: 5678,
            stop_reason: StopReason::EndTurn,
            advisor: Default::default(),
        })),
    ]]);
    let client = LlmClient::with_provider(provider);

    let (tx, rx) = mpsc::channel::<Event<()>>(64);
    let events: Option<EventStream<()>> = Some(tx);

    let _ = client
        .run_with_tools::<(), _>(
            "system prompt",
            "hi",
            &[],
            |_, _| Box::pin(async { Ok(Box::new(Value::Null) as Box<dyn ToolOutput>) }),
            &events,
            ToolLoopConfig {
                max_tool_rounds: 1,
                state: "test".into(),
                ..Default::default()
            },
        )
        .await
        .expect("run_with_tools should succeed");
    drop(events);

    let mut creation_seen = 0usize;
    let mut read_seen = 0usize;
    let mut llm_end_seen = false;
    let mut rx = rx;
    while let Some(ev) = rx.recv().await {
        if let Event::Core(CoreEvent::LlmEnd {
            cache_creation_input_tokens,
            cache_read_input_tokens,
            output_tokens,
            ..
        }) = ev
        {
            creation_seen = cache_creation_input_tokens;
            read_seen = cache_read_input_tokens;
            assert_eq!(output_tokens, 20);
            llm_end_seen = true;
        }
    }
    assert!(llm_end_seen, "expected at least one LlmEnd event");
    assert_eq!(creation_seen, 1234);
    assert_eq!(read_seen, 5678);
}

// ── Per-round message-level cache breakpoint ──────────────────────────────

/// `build_request_body` must mark the last content block of the last
/// message so Round N reads the growing tool_use / tool_result history
/// from cache instead of re-paying for it every round.
#[test]
fn build_request_body_marks_last_message_block_with_cache_control() {
    let messages = vec![json!({
        "role": "user",
        "content": [
            {"type": "tool_result", "tool_use_id": "t1", "content": "row 1"},
            {"type": "tool_result", "tool_use_id": "t2", "content": "row 2"},
        ]
    })];
    let provider = super::anthropic::AnthropicProvider::new("k", "claude-sonnet-4-6");
    let body = provider.build_request_body(
        "system",
        "",
        &messages,
        &[],
        &super::ThinkingConfig::Disabled,
        None,
        None,
    );
    let blocks = body["messages"][0]["content"]
        .as_array()
        .expect("content remains an array");
    assert!(
        blocks[0].get("cache_control").is_none(),
        "non-last block must NOT carry cache_control"
    );
    assert_eq!(
        blocks[1]["cache_control"],
        json!({"type": "ephemeral"}),
        "last block must carry cache_control"
    );
}

/// String-valued `content` (e.g. the initial user question) must be
/// lifted to a one-element block array so the cache marker can attach.
#[test]
fn build_request_body_lifts_string_content_to_blocks_for_caching() {
    let messages = vec![json!({"role": "user", "content": "what is 2+2?"})];
    let provider = super::anthropic::AnthropicProvider::new("k", "claude-sonnet-4-6");
    let body = provider.build_request_body(
        "system",
        "",
        &messages,
        &[],
        &super::ThinkingConfig::Disabled,
        None,
        None,
    );
    let content = body["messages"][0]["content"]
        .as_array()
        .expect("string content was lifted to an array");
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"].as_str(), Some("text"));
    assert_eq!(content[0]["text"].as_str(), Some("what is 2+2?"));
    assert_eq!(content[0]["cache_control"], json!({"type": "ephemeral"}));
}

/// Critical guarantee: ONLY the last message gets a cache breakpoint.
/// Marking earlier messages would silently exhaust Anthropic's 4-slot
/// budget — already two slots are taken by system + last tool, leaving
/// just one for the message tail.
#[test]
fn build_request_body_marks_only_last_message_not_earlier_ones() {
    let messages = vec![
        json!({"role": "user", "content": "initial question"}),
        json!({
            "role": "assistant",
            "content": [{"type": "tool_use", "id": "t1", "name": "fn", "input": {}}]
        }),
        json!({
            "role": "user",
            "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "row"}]
        }),
    ];
    let provider = super::anthropic::AnthropicProvider::new("k", "claude-sonnet-4-6");
    let body = provider.build_request_body(
        "sys",
        "",
        &messages,
        &[],
        &super::ThinkingConfig::Disabled,
        None,
        None,
    );
    // First message kept its string form (or, if lifted, would not carry
    // cache_control on the lifted block).  Either way: no breakpoint.
    let m0_content = &body["messages"][0]["content"];
    if let Some(arr) = m0_content.as_array() {
        for block in arr {
            assert!(
                block.get("cache_control").is_none(),
                "msg[0] block must NOT carry cache_control: {block}"
            );
        }
    } else {
        assert!(
            m0_content.is_string(),
            "msg[0] should still be a plain string"
        );
    }
    // Second message's tool_use block must NOT be marked.
    let m1_block0 = &body["messages"][1]["content"][0];
    assert!(
        m1_block0.get("cache_control").is_none(),
        "msg[1] block must NOT carry cache_control"
    );
    // Only the last message's last block should be marked.
    let last_block = &body["messages"][2]["content"][0];
    assert_eq!(
        last_block["cache_control"],
        json!({"type": "ephemeral"}),
        "only the last message's last block must carry cache_control"
    );
}

/// `cache_control` on a thinking block is rejected by the API, so the
/// marker must skip over thinking entries and attach to the nearest
/// non-thinking block (or no-op if none exists).
#[test]
fn build_request_body_skips_thinking_block_when_marking_last_message() {
    // Hypothetical malformed-but-tolerated history: assistant turn ending
    // in a thinking block.  The marker must skip the thinking and land on
    // the text block before it.
    let messages = vec![json!({
        "role": "assistant",
        "content": [
            {"type": "text", "text": "let me think"},
            {"type": "thinking", "thinking": "...", "signature": "sig"},
        ]
    })];
    let provider = super::anthropic::AnthropicProvider::new("k", "claude-sonnet-4-6");
    let body = provider.build_request_body(
        "system",
        "",
        &messages,
        &[],
        &super::ThinkingConfig::Disabled,
        None,
        None,
    );
    let blocks = body["messages"][0]["content"].as_array().unwrap();
    assert!(
        blocks[1].get("cache_control").is_none(),
        "thinking block must NEVER carry cache_control"
    );
    assert_eq!(
        blocks[0]["cache_control"],
        json!({"type": "ephemeral"}),
        "marker falls back to the preceding non-thinking block"
    );
}
