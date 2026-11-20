//! Tool-call id uniqueness and trace parity (`ids.rs`, `trace.rs`).

use std::sync::{Arc, Mutex};

use agentic_core::react::{Decision, Entry, ToolCallReq, Transcript, Turn, TurnUsage};
use serde_json::json;
use tracing::field::{Field, Visit};
use tracing_subscriber::layer::{Context, SubscriberExt as _};

use super::EngineConfig;
use super::mock::{Round, assistant};
use super::tests::{decide, done, engine, question, text, tool_call};
use crate::{Chunk, ContentBlock, StopReason, ToolCallChunk};

fn call_tools(d: Decision) -> Turn {
    match d {
        Decision::CallTools(t) => t,
        other => panic!("expected CallTools, got {other:?}"),
    }
}

fn ids(t: &Turn) -> Vec<&str> {
    t.tool_calls.iter().map(|c| c.id.as_str()).collect()
}

fn native_ids(t: &Turn) -> Vec<String> {
    let blocks = t.native["content"].as_array().unwrap();
    blocks
        .iter()
        .filter(|b| b["type"] == "tool_use")
        .map(|b| b["id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn missing_tool_call_ids_are_synthesized() {
    let (e, _) = engine(
        vec![Round::Chunks(vec![
            tool_call(""),
            tool_call(""),
            done(StopReason::ToolUse),
        ])],
        EngineConfig::default(),
    );
    let turn = call_tools(decide(&e, &question("q")).await.0.unwrap());
    assert_eq!(ids(&turn), ["call_0_0", "call_0_1"]);
    assert_eq!(native_ids(&turn), ["call_0_0", "call_0_1"]);
}

#[tokio::test]
async fn duplicate_tool_call_ids_are_made_unique() {
    let (e, _) = engine(
        vec![Round::Chunks(vec![
            tool_call("a"),
            tool_call("a"),
            done(StopReason::ToolUse),
        ])],
        EngineConfig::default(),
    );
    let turn = call_tools(decide(&e, &question("q")).await.0.unwrap());
    assert_eq!(ids(&turn), ["a", "call_0_1"]);
    assert_eq!(native_ids(&turn), ["a", "call_0_1"]);
}

#[tokio::test]
async fn an_id_used_by_an_earlier_turn_is_rewritten_with_its_turn_index() {
    let prior = Turn {
        text: String::new(),
        tool_calls: vec![ToolCallReq {
            id: "a".into(),
            name: "query".into(),
            input: json!({}),
        }],
        native: assistant(&[ContentBlock::Text { text: "x".into() }]),
        usage: TurnUsage::default(),
    };
    let t = Transcript {
        system: "sys".into(),
        entries: vec![
            Entry::User("q".into()),
            Entry::Assistant(prior),
            Entry::ToolResult {
                call_id: "a".into(),
                text: "ok".into(),
                is_error: false,
            },
        ],
    };
    let (e, _) = engine(
        vec![Round::Chunks(vec![
            tool_call("a"),
            done(StopReason::ToolUse),
        ])],
        EngineConfig::default(),
    );
    let turn = call_tools(decide(&e, &t).await.0.unwrap());
    assert_eq!(ids(&turn), ["call_1_0"]);
    assert_eq!(native_ids(&turn), ["call_1_0"]);
}

#[tokio::test]
async fn a_native_openai_call_id_is_rewritten_too() {
    let chunk = Chunk::ToolCall(ToolCallChunk {
        id: String::new(),
        name: "query".into(),
        input: json!({}),
        provider_data: Some(json!({"type": "function_call", "id": "fc_1", "call_id": ""})),
    });
    let (e, _) = engine(
        vec![Round::Chunks(vec![Ok(chunk), done(StopReason::ToolUse)])],
        EngineConfig::default(),
    );
    let turn = call_tools(decide(&e, &question("q")).await.0.unwrap());
    assert_eq!(ids(&turn), ["call_0_0"]);
}

#[tokio::test]
async fn the_date_hint_reaches_the_provider() {
    let cfg = EngineConfig {
        system_date_hint: Some("Today is 2026-11-20.".into()),
        ..Default::default()
    };
    let (e, seen) = engine(
        vec![Round::Chunks(vec![text("hi"), done(StopReason::EndTurn)])],
        cfg,
    );
    decide(&e, &question("q")).await.0.unwrap();
    assert_eq!(seen.lock().unwrap().date_suffixes, ["Today is 2026-11-20."]);
}

// ── tracing ───────────────────────────────────────────────────────────────

#[derive(Default)]
struct Fields(Vec<(String, String)>);

impl Visit for Fields {
    fn record_debug(&mut self, f: &Field, v: &dyn std::fmt::Debug) {
        self.0.push((f.name().into(), format!("{v:?}")));
    }
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<(String, Vec<(String, String)>)>>>);

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Capture {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        let mut f = Fields::default();
        event.record(&mut f);
        self.0
            .lock()
            .unwrap()
            .push((event.metadata().name().to_string(), f.0));
    }
}

#[tokio::test]
async fn engine_emits_llm_usage_trace_event() {
    let capture = Capture::default();
    let sub = tracing_subscriber::registry().with(capture.clone());
    let _guard = tracing::subscriber::set_default(sub);
    let (e, _) = engine(
        vec![Round::Chunks(vec![text("hi"), done(StopReason::EndTurn)])],
        EngineConfig::default(),
    );
    decide(&e, &question("q")).await.0.unwrap();
    let events = capture.0.lock().unwrap();
    let field = |ev: &str, name: &str| {
        events
            .iter()
            .find(|(n, _)| n == ev)
            .and_then(|(_, fs)| fs.iter().find(|(k, _)| k == name))
            .map(|(_, v)| v.clone())
    };
    assert_eq!(field("llm.usage", "prompt_tokens").as_deref(), Some("10"));
    assert_eq!(
        field("llm.usage", "completion_tokens").as_deref(),
        Some("5")
    );
    assert_eq!(field("llm.usage", "total_tokens").as_deref(), Some("15"));
    assert_eq!(field("llm.output", "text").as_deref(), Some("hi"));
}
