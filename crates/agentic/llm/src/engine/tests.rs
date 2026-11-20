//! `LlmEngine` against a scripted provider.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use agentic_core::react::{
    Decision, Engine, EngineError, Entry, PriorTurn, Transcript, Turn, TurnUsage,
};
use serde_json::json;
use tokio::sync::mpsc;

use super::mock::{MockProvider, Round, Seen, assistant, tool_results};
use super::{EngineConfig, LlmEngine};
use crate::{Chunk, ContentBlock, LlmClient, LlmError, StopReason, ToolCallChunk, Usage};

// ── Helpers ───────────────────────────────────────────────────────────────

pub(super) fn engine(rounds: Vec<Round>, cfg: EngineConfig) -> (LlmEngine, Arc<Mutex<Seen>>) {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let provider = MockProvider {
        rounds: Mutex::new(rounds.into()),
        seen: seen.clone(),
    };
    (
        LlmEngine::new(LlmClient::with_provider(provider), cfg),
        seen,
    )
}

pub(super) fn done(stop_reason: StopReason) -> Result<Chunk, LlmError> {
    Ok(Chunk::Done(Usage {
        input_tokens: 10,
        output_tokens: 5,
        stop_reason,
        ..Default::default()
    }))
}

pub(super) fn text(t: &str) -> Result<Chunk, LlmError> {
    Ok(Chunk::Text(t.into()))
}

pub(super) fn tool_call(id: &str) -> Result<Chunk, LlmError> {
    Ok(Chunk::ToolCall(ToolCallChunk {
        id: id.into(),
        name: "query".into(),
        input: json!({"sql": id}),
        provider_data: None,
    }))
}

fn thinking_block() -> ContentBlock {
    ContentBlock::Thinking {
        provider_data: json!({"type": "thinking", "thinking": "hmm", "signature": "sig"}),
    }
}

pub(super) fn question(q: &str) -> Transcript {
    Transcript {
        system: "sys".into(),
        entries: vec![Entry::User(q.into())],
    }
}

/// Runs one `decide` and returns it with the event types it emitted.
pub(super) async fn decide(
    e: &LlmEngine,
    t: &Transcript,
) -> (Result<Decision, EngineError>, Vec<String>) {
    let (tx, mut rx) = mpsc::channel(256);
    let out = e.decide(t, &[], &tx).await;
    drop(tx);
    let mut types = Vec::new();
    while let Ok((event_type, _)) = rx.try_recv() {
        types.push(event_type);
    }
    (out, types)
}

fn reply(d: Result<Decision, EngineError>) -> Turn {
    match d {
        Ok(Decision::Reply(t)) => t,
        other => panic!("expected Reply, got {other:?}"),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_text_only_stream_is_a_reply_with_usage_and_cost() {
    let (e, _) = engine(
        vec![Round::Chunks(vec![text("hi"), done(StopReason::EndTurn)])],
        EngineConfig::default(),
    );
    let (out, events) = decide(&e, &question("q")).await;
    let turn = reply(out);
    assert_eq!(turn.text, "hi");
    assert_eq!(turn.usage.input_tokens, 10);
    assert_eq!(turn.usage.output_tokens, 5);
    assert_eq!(turn.usage.model, "claude-sonnet-4-6");
    assert!(turn.usage.cost_usd.is_some());
    assert_eq!(
        turn.native,
        assistant(&[ContentBlock::Text { text: "hi".into() }])
    );
    // The driver closes the turn with LlmEnd; the engine must not.
    assert_eq!(events, ["llm_start", "llm_token"]);
}

#[tokio::test]
async fn tool_call_chunks_become_call_tools_with_ids() {
    let (e, _) = engine(
        vec![Round::Chunks(vec![
            tool_call("c1"),
            tool_call("c2"),
            done(StopReason::ToolUse),
        ])],
        EngineConfig::default(),
    );
    let turn = match decide(&e, &question("q")).await.0 {
        Ok(Decision::CallTools(t)) => t,
        other => panic!("expected CallTools, got {other:?}"),
    };
    let ids: Vec<&str> = turn.tool_calls.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["c1", "c2"]);
    assert_eq!(turn.tool_calls[0].name, "query");
    assert_eq!(turn.tool_calls[1].input, json!({"sql": "c2"}));
}

#[tokio::test]
async fn raw_thinking_blocks_are_kept_in_native_in_order() {
    let (e, _) = engine(
        vec![Round::Chunks(vec![
            Ok(Chunk::RawBlock(thinking_block())),
            text("let me look"),
            tool_call("c1"),
            done(StopReason::ToolUse),
        ])],
        EngineConfig::default(),
    );
    let turn = match decide(&e, &question("q")).await.0 {
        Ok(Decision::CallTools(t)) => t,
        other => panic!("expected CallTools, got {other:?}"),
    };
    let expected = assistant(&[
        ContentBlock::Text {
            text: "let me look".into(),
        },
        thinking_block(),
        ContentBlock::ToolUse {
            id: "c1".into(),
            name: "query".into(),
            input: json!({"sql": "c1"}),
            provider_data: None,
        },
    ]);
    assert_eq!(turn.native, expected);
}

#[tokio::test]
async fn a_thinking_only_empty_turn_is_continued() {
    let (e, seen) = engine(
        vec![
            Round::Chunks(vec![
                Ok(Chunk::ThinkingSummary("hmm".into())),
                Ok(Chunk::RawBlock(thinking_block())),
                done(StopReason::EndTurn),
            ]),
            Round::Chunks(vec![text("ok"), done(StopReason::EndTurn)]),
        ],
        EngineConfig::default(),
    );
    let turn = reply(decide(&e, &question("q")).await.0);
    assert_eq!(turn.text, "ok");
    // Both streams are billed.
    assert_eq!(turn.usage.input_tokens, 20);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.messages.len(), 2);
    let second = &seen.messages[1];
    let sentinel = assistant(&[
        thinking_block(),
        ContentBlock::Text {
            text: String::new(),
        },
    ]);
    assert_eq!(second[1], sentinel);
    assert_eq!(second[2], json!({"role": "user", "content": "Continue."}));
}

/// A thinking-only stream with no summary text is still continued.
async fn assert_continued_after(block: ContentBlock) {
    let (e, seen) = engine(
        vec![
            Round::Chunks(vec![Ok(Chunk::RawBlock(block)), done(StopReason::EndTurn)]),
            Round::Chunks(vec![text("ok"), done(StopReason::EndTurn)]),
        ],
        EngineConfig::default(),
    );
    assert_eq!(reply(decide(&e, &question("q")).await.0).text, "ok");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.messages.len(), 2);
    assert_eq!(
        seen.messages[1].last(),
        Some(&json!({"role": "user", "content": "Continue."}))
    );
}

#[tokio::test]
async fn a_thinking_block_without_summary_text_is_continued() {
    assert_continued_after(thinking_block()).await;
}

#[tokio::test]
async fn a_redacted_thinking_block_alone_is_continued() {
    let redacted = ContentBlock::RedactedThinking {
        provider_data: json!({"type": "redacted_thinking", "data": "opaque"}),
    };
    assert_continued_after(redacted).await;
}

#[tokio::test(start_paused = true)]
async fn rate_limit_is_retried_then_succeeds() {
    let rate_limited = LlmError::RateLimit {
        message: "429".into(),
        retry_after: Some(Duration::ZERO),
    };
    let (e, seen) = engine(
        vec![
            Round::Chunks(vec![text("partial"), Err(rate_limited)]),
            Round::Chunks(vec![text("ok"), done(StopReason::EndTurn)]),
        ],
        EngineConfig::default(),
    );
    let (out, events) = decide(&e, &question("q")).await;
    let turn = reply(out);
    // One turn, from the successful attempt only: no "partialok".
    assert_eq!(turn.text, "ok");
    assert_eq!(turn.usage.input_tokens, 10);
    assert_eq!(seen.lock().unwrap().messages.len(), 2);
    assert_eq!(events.iter().filter(|t| *t == "llm_start").count(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_transient_open_error_is_retried() {
    let (e, seen) = engine(
        vec![
            Round::Open(LlmError::Transient("reset".into())),
            Round::Chunks(vec![text("ok"), done(StopReason::EndTurn)]),
        ],
        EngineConfig::default(),
    );
    assert_eq!(reply(decide(&e, &question("q")).await.0).text, "ok");
    assert_eq!(seen.lock().unwrap().messages.len(), 2);
}

#[tokio::test(start_paused = true)]
async fn rate_limit_retries_are_bounded() {
    let limited = || {
        Round::Open(LlmError::RateLimit {
            message: "429".into(),
            retry_after: Some(Duration::from_secs(7)),
        })
    };
    let cfg = EngineConfig {
        max_rate_limit_retries: 1,
        ..Default::default()
    };
    let (e, seen) = engine(vec![limited(), limited()], cfg);
    let out = decide(&e, &question("q")).await.0;
    assert!(
        matches!(
            out,
            Err(EngineError::RateLimited {
                retry_after: Some(7)
            })
        ),
        "{out:?}"
    );
    assert_eq!(seen.lock().unwrap().messages.len(), 2);
}

#[tokio::test]
async fn an_auth_error_is_fatal_without_retry() {
    let (e, seen) = engine(
        vec![
            Round::Open(LlmError::Auth("bad key".into())),
            Round::Chunks(vec![text("never"), done(StopReason::EndTurn)]),
        ],
        EngineConfig::default(),
    );
    let out = decide(&e, &question("q")).await.0;
    assert!(matches!(out, Err(EngineError::Fatal(_))), "{out:?}");
    assert_eq!(seen.lock().unwrap().messages.len(), 1);
}

#[tokio::test]
async fn truncated_text_retries_once_doubled_then_is_max_tokens() {
    let (e, seen) = engine(
        vec![
            Round::Chunks(vec![text("cut"), done(StopReason::MaxTokens)]),
            Round::Chunks(vec![text("cut again"), done(StopReason::MaxTokens)]),
        ],
        EngineConfig::default(),
    );
    let out = decide(&e, &question("q")).await.0;
    match out {
        Err(EngineError::MaxTokens { partial_text }) => assert_eq!(partial_text, "cut again"),
        other => panic!("expected MaxTokens, got {other:?}"),
    }
    let seen = seen.lock().unwrap();
    assert_eq!(seen.max_tokens, [None, Some(2 * 4096)]);
    // A re-stream, not a continuation: the same messages both times.
    assert_eq!(seen.messages[0], seen.messages[1]);
}

#[tokio::test]
async fn transcript_renders_tool_results_through_the_provider() {
    let native = json!({"role": "assistant", "content": [{"type": "native-marker"}]});
    let prior = Turn {
        text: "looking".into(),
        tool_calls: vec![],
        native: native.clone(),
        usage: TurnUsage::default(),
    };
    let result = |id: &str, err: bool| Entry::ToolResult {
        call_id: id.into(),
        text: format!("out {id}"),
        is_error: err,
    };
    let t = Transcript {
        system: "sys".into(),
        entries: vec![
            Entry::User("q".into()),
            Entry::Assistant(prior),
            result("c1", false),
            result("c2", true),
        ],
    };
    let (e, seen) = engine(
        vec![Round::Chunks(vec![text("ok"), done(StopReason::EndTurn)])],
        EngineConfig::default(),
    );
    reply(decide(&e, &t).await.0);
    let mut expected = vec![json!({"role": "user", "content": "q"}), native];
    expected.extend(tool_results(&[
        ("c1".into(), "out c1".into(), false),
        ("c2".into(), "out c2".into(), true),
    ]));
    assert_eq!(seen.lock().unwrap().messages[0], expected);
}

#[tokio::test]
async fn a_seeded_assistant_turn_with_null_native_renders_as_text() {
    let prior = [PriorTurn {
        question: "q1".into(),
        answer: "a1".into(),
        results_summary: String::new(),
    }];
    let t = Transcript::seeded("sys".into(), &prior, "q2".into());
    let (e, seen) = engine(
        vec![Round::Chunks(vec![text("ok"), done(StopReason::EndTurn)])],
        EngineConfig::default(),
    );
    reply(decide(&e, &t).await.0);
    let sent = &seen.lock().unwrap().messages[0];
    assert_eq!(sent.len(), 3);
    assert_eq!(
        sent[1],
        assistant(&[ContentBlock::Text { text: "a1".into() }])
    );
    assert!(sent.iter().all(|m| !m.is_null()));
}
