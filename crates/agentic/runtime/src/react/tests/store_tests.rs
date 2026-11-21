use std::sync::Arc;

use agentic_core::react::{Decision, Entry, Tool, Toolset, Transcript};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::super::AskUser;
use super::super::testkit::*;
use super::super::{
    Budgets, InMemoryStore, LoopConfig, ReactEvent, StoreError, TranscriptStore, rebuild, run_loop,
};

fn seed() -> Transcript {
    Transcript {
        system: "sys".into(),
        entries: vec![Entry::User("q".into())],
    }
}

async fn drive(engine: &ScriptedEngine) -> Vec<(String, Value)> {
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(EchoRead), Arc::new(AskUser)];
    let toolset = Toolset {
        system_prompt: "sys".into(),
        tools,
    };
    let cfg = LoopConfig {
        budgets: Budgets::default(),
        run_id: "run1".into(),
        results: Arc::new(NoResults),
        on_done: None,
    };
    let (tx, mut rx) = mpsc::channel(256);
    run_loop(
        engine,
        &toolset,
        seed(),
        &cfg,
        &tx,
        &CancellationToken::new(),
    )
    .await;
    drop(tx);
    let mut rows = Vec::new();
    while let Some(r) = rx.recv().await {
        rows.push(r);
    }
    rows
}

fn echo(id: &str) -> agentic_core::react::ToolCallReq {
    call(id, "echo", json!({"text": id}))
}

#[tokio::test]
async fn react_rebuild_reproduces_the_transcript_the_engine_last_saw() {
    let e = ScriptedEngine::new(vec![
        call_tools(vec![echo("c1"), echo("c2")]),
        call_tools(vec![echo("c3")]),
        reply("final"),
    ]);
    let rows = drive(&e).await;
    let store = InMemoryStore::new(rows);
    let rebuilt = rebuild(seed(), &store.load("run1").await.unwrap(), None).unwrap();

    let mut expected = e.seen.lock().unwrap().last().unwrap().entries.clone();
    expected.push(Entry::Assistant(turn("final", vec![])));
    assert_eq!(rebuilt.transcript.entries, expected);
    assert_eq!(rebuilt.recorded.len(), 3);
    assert!(rebuilt.pending_calls.is_empty());
}

#[tokio::test]
async fn react_native_assistant_turns_survive_rebuild_verbatim() {
    let native = json!([{"type":"thinking","signature":"sig","thinking":"hm \u{2026} \"q\""}]);
    let mut t = turn("", vec![echo("c1")]);
    t.native = native.clone();
    let e = ScriptedEngine::new(vec![Ok(Decision::CallTools(t)), reply("ok")]);
    let rows = drive(&e).await;
    let rebuilt = rebuild(seed(), &rows, None).unwrap();
    let Entry::Assistant(first) = &rebuilt.transcript.entries[1] else {
        panic!("expected assistant entry");
    };
    assert_eq!(first.native, native);
    assert_eq!(
        serde_json::to_string(&first.native).unwrap(),
        serde_json::to_string(&native).unwrap()
    );
}

#[tokio::test]
async fn react_core_and_unknown_rows_are_ignored() {
    let e = ScriptedEngine::new(vec![call_tools(vec![echo("c1")]), reply("ok")]);
    let rows = drive(&e).await;
    let clean = rebuild(seed(), &rows, None).unwrap();

    let mut noisy = vec![
        ("llm_token".to_string(), json!({"token": "x"})),
        ("tool_call".to_string(), json!({"name": "echo"})),
        (
            "future_thing".to_string(),
            json!({"event_type": "future_thing"}),
        ),
    ];
    for r in &rows {
        noisy.push(r.clone());
        noisy.push(("future_thing".to_string(), json!({"a": 1})));
    }
    let b = rebuild(seed(), &noisy, None).unwrap();
    assert_eq!(b.transcript.entries, clean.transcript.entries);
    assert_eq!(b.recorded, clean.recorded);
    assert_eq!(b.pending_calls, clean.pending_calls);
}

#[tokio::test]
async fn react_pending_calls_are_the_last_turns_unanswered_calls() {
    let e = ScriptedEngine::new(vec![
        call_tools(vec![echo("c1")]),
        call_tools(vec![
            echo("c2"),
            call("c3", "ask_user", json!({"question": "?"})),
        ]),
    ]);
    let rows = drive(&e).await;
    let b = rebuild(seed(), &rows, None).unwrap();
    // c1 answered in an earlier turn, c2 answered, c3 suspended: only c3 pends.
    let ids: Vec<_> = b.pending_calls.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["c3"]);
    assert!(b.recorded.contains_key("c1") && b.recorded.contains_key("c2"));
}

#[test]
fn react_malformed_react_rows_are_errors_not_skipped() {
    let rows = vec![("react_turn".to_string(), json!({"turn": 5}))];
    let err = rebuild(seed(), &rows, None).unwrap_err();
    assert!(
        matches!(err, StoreError::Malformed { ref event_type, .. } if event_type == "react_turn")
    );
}

#[tokio::test]
async fn react_an_unknown_react_tag_is_skipped_not_malformed() {
    let e = ScriptedEngine::new(vec![call_tools(vec![echo("c1")]), reply("ok")]);
    let rows = drive(&e).await;
    let clean = rebuild(seed(), &rows, None).unwrap();
    let mut noisy = rows.clone();
    noisy.insert(1, ("react_future_thing".to_string(), json!({"turn": 5})));
    let b = rebuild(seed(), &noisy, None).unwrap();
    assert_eq!(b.transcript.entries, clean.transcript.entries);
    assert_eq!(b.pending_calls, clean.pending_calls);
}

#[test]
fn react_a_duplicate_tool_result_keeps_the_first() {
    let result = |text: &str| {
        ReactEvent::ReactToolResult {
            call_id: "c1".into(),
            name: "echo".into(),
            text: text.into(),
            is_error: false,
            value: Value::Null,
        }
        .to_row()
    };
    let rows = vec![
        ReactEvent::ReactTurn {
            turn: turn("", vec![echo("c1")]),
        }
        .to_row(),
        result("first"),
        result("second"),
    ];
    let b = rebuild(seed(), &rows, None).unwrap();
    let results: Vec<_> = b
        .transcript
        .entries
        .iter()
        .filter_map(|x| match x {
            Entry::ToolResult { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(results, ["first"]);
    assert!(matches!(&b.recorded["c1"], Entry::ToolResult { text, .. } if text == "first"));
    assert!(b.pending_calls.is_empty());
}
