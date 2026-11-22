//! `LlmEngine` with Anthropic's advisor: paused turns, stream order, usage.

use agentic_core::react::{Decision, EngineError};
use serde_json::json;

use super::EngineConfig;
use super::mock::{Round, assistant};
use super::tests::{decide, engine, question, text, tool_call};
use crate::{AdvisorUsage, Chunk, ContentBlock, LlmError, StopReason, Usage};

fn server_tool_use() -> ContentBlock {
    ContentBlock::ServerTool {
        provider_data: json!({"type": "server_tool_use", "id": "srvtoolu_1", "name": "advisor", "input": {}}),
    }
}

fn advisor_result() -> ContentBlock {
    ContentBlock::ServerTool {
        provider_data: json!({
            "type": "advisor_tool_result",
            "tool_use_id": "srvtoolu_1",
            "content": {"type": "advisor_redacted_result", "encrypted_content": "e"}
        }),
    }
}

fn raw(b: ContentBlock) -> Result<Chunk, LlmError> {
    Ok(Chunk::RawBlock(b))
}

fn done_with_advisor(stop_reason: StopReason, calls: u32) -> Result<Chunk, LlmError> {
    Ok(Chunk::Done(Usage {
        input_tokens: 10,
        output_tokens: 5,
        stop_reason,
        advisor: AdvisorUsage {
            calls,
            model: "claude-opus-4-8".into(),
            input_tokens: 800 * calls as usize,
            output_tokens: 600 * calls as usize,
            ..Default::default()
        },
        ..Default::default()
    }))
}

/// A paused stream goes back as the assistant message with nothing after
/// it, and the returned turn holds both streams' blocks in stream order.
#[tokio::test]
async fn a_paused_turn_is_resumed_without_a_user_message_and_kept_whole() {
    let (e, seen) = engine(
        vec![
            Round::Chunks(vec![
                text("Checking."),
                raw(server_tool_use()),
                done_with_advisor(StopReason::PauseTurn, 0),
            ]),
            Round::Chunks(vec![
                raw(advisor_result()),
                text("Answer."),
                done_with_advisor(StopReason::EndTurn, 1),
            ]),
        ],
        EngineConfig::default(),
    );
    let (out, _) = decide(&e, &question("q")).await;
    let Ok(Decision::Reply(turn)) = out else {
        panic!("expected a reply, got {out:?}")
    };
    let seen = seen.lock().unwrap();
    assert_eq!(seen.messages.len(), 2);
    let resumed = &seen.messages[1];
    assert_eq!(
        resumed.len(),
        seen.messages[0].len() + 1,
        "only the paused turn is appended"
    );
    assert_eq!(
        resumed.last().unwrap(),
        &assistant(&[
            ContentBlock::Text {
                text: "Checking.".into()
            },
            server_tool_use()
        ])
    );
    assert_eq!(turn.text, "Checking.Answer.");
    assert_eq!(
        turn.native,
        assistant(&[
            ContentBlock::Text {
                text: "Checking.".into()
            },
            server_tool_use(),
            advisor_result(),
            ContentBlock::Text {
                text: "Answer.".into()
            },
        ])
    );
    assert_eq!(turn.usage.advisor_calls, 1);
    assert_eq!(
        (
            turn.usage.advisor_input_tokens,
            turn.usage.advisor_output_tokens
        ),
        (800, 600)
    );
    assert_eq!(
        (turn.usage.input_tokens, turn.usage.output_tokens),
        (20, 10)
    );
}

/// Text written after the advice stays after it; without advisor blocks
/// the old text-first order holds.
#[tokio::test]
async fn advisor_turns_keep_stream_order_and_others_put_text_first() {
    let (e, _) = engine(
        vec![Round::Chunks(vec![
            raw(server_tool_use()),
            raw(advisor_result()),
            text("Run it."),
            tool_call("t1"),
            done_with_advisor(StopReason::ToolUse, 1),
        ])],
        EngineConfig::default(),
    );
    let (out, _) = decide(&e, &question("q")).await;
    let Ok(Decision::CallTools(turn)) = out else {
        panic!("expected tool calls, got {out:?}")
    };
    let content = turn.native["content"].as_array().unwrap();
    let kinds: Vec<&str> = content
        .iter()
        .map(|b| b["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        ["server_tool_use", "advisor_tool_result", "text", "tool_use"]
    );

    let (e, _) = engine(
        vec![Round::Chunks(vec![
            tool_call("t1"),
            text("Run it."),
            done_with_advisor(StopReason::ToolUse, 0),
        ])],
        EngineConfig::default(),
    );
    let (out, _) = decide(&e, &question("q")).await;
    let Ok(Decision::CallTools(turn)) = out else {
        panic!("expected tool calls, got {out:?}")
    };
    let kinds: Vec<&str> = turn.native["content"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["type"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["text", "tool_use"]);
}

/// A paused stream with thinking and no text is resumed, not "Continue."d.
#[tokio::test]
async fn a_thinking_only_pause_is_resumed_not_continued() {
    let thinking = ContentBlock::Thinking {
        provider_data: json!({"type": "thinking", "thinking": "", "signature": "s"}),
    };
    let (e, seen) = engine(
        vec![
            Round::Chunks(vec![
                raw(thinking),
                raw(server_tool_use()),
                done_with_advisor(StopReason::PauseTurn, 0),
            ]),
            Round::Chunks(vec![
                raw(advisor_result()),
                text("Done."),
                done_with_advisor(StopReason::EndTurn, 1),
            ]),
        ],
        EngineConfig::default(),
    );
    let (out, _) = decide(&e, &question("q")).await;
    assert!(matches!(out, Ok(Decision::Reply(_))), "{out:?}");
    let seen = seen.lock().unwrap();
    let last = seen.messages[1].last().unwrap();
    assert_eq!(
        last["role"], "assistant",
        "no Continue. user message: {last}"
    );
}

#[tokio::test]
async fn a_turn_that_never_stops_pausing_fails() {
    let rounds = (0..=super::MAX_PAUSES)
        .map(|_| {
            Round::Chunks(vec![
                raw(server_tool_use()),
                done_with_advisor(StopReason::PauseTurn, 0),
            ])
        })
        .collect();
    let (e, _) = engine(rounds, EngineConfig::default());
    let (out, _) = decide(&e, &question("q")).await;
    assert!(
        matches!(&out, Err(EngineError::Fatal(m)) if m.contains("paused")),
        "{out:?}"
    );
}

/// An advisor model the price table does not know still costs something:
/// the executor's cost is kept and the advisor is priced at the dearest
/// rate, so the react cost cap cannot read the turn as free.
#[tokio::test]
async fn an_unpriced_advisor_never_makes_a_turn_free() {
    let usage = |model: &str| {
        Ok(Chunk::Done(Usage {
            input_tokens: 10,
            output_tokens: 5,
            stop_reason: StopReason::EndTurn,
            advisor: AdvisorUsage {
                calls: 1,
                model: model.into(),
                input_tokens: 1_000_000,
                output_tokens: 0,
                ..Default::default()
            },
            ..Default::default()
        }))
    };
    let cost = |model: &str| {
        let (e, _) = engine(
            vec![Round::Chunks(vec![text("a"), usage(model)])],
            EngineConfig::default(),
        );
        async move {
            let (out, _) = decide(&e, &question("q")).await;
            let Ok(Decision::Reply(turn)) = out else {
                panic!("{out:?}")
            };
            turn.usage.cost_usd
        }
    };
    let unknown = cost("claude-zephyr-9").await.expect("priced");
    let dearest = cost("claude-opus-4-1").await.expect("priced");
    assert!((unknown - dearest).abs() < 1e-9, "{unknown} vs {dearest}");
    assert!(unknown >= 15.0, "1M advisor input at $15/M: {unknown}");
}
