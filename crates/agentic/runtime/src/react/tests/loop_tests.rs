use std::sync::Arc;

use agentic_core::react::{Engine, EngineError, Entry, Tool, Toolset, Transcript};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::super::testkit::*;
use super::super::{Budgets, LoopConfig, LoopOutcome, ReactEvent, run_loop};

fn toolset() -> Toolset {
    let tools: Vec<Arc<dyn Tool>> =
        vec![Arc::new(EchoRead), Arc::new(FailingRead), Arc::new(AskUser)];
    Toolset {
        system_prompt: "sys".into(),
        tools,
    }
}

fn cfg() -> LoopConfig {
    LoopConfig {
        budgets: Budgets::default(),
        run_id: "run1".into(),
        results: Arc::new(NoResults),
    }
}

async fn drive(engine: &dyn Engine) -> (LoopOutcome, Vec<(String, Value)>) {
    let (tx, mut rx) = mpsc::channel(256);
    let t = Transcript {
        system: "sys".into(),
        entries: vec![Entry::User("q".into())],
    };
    let out = run_loop(
        engine,
        &toolset(),
        t,
        &cfg(),
        &tx,
        &CancellationToken::new(),
    )
    .await;
    drop(tx);
    let mut rows = Vec::new();
    while let Some(r) = rx.recv().await {
        rows.push(r);
    }
    (out, rows)
}

fn count(rows: &[(String, Value)], t: &str) -> usize {
    rows.iter().filter(|(e, _)| e == t).count()
}

fn last_result(e: &ScriptedEngine, which: usize) -> Entry {
    e.seen.lock().unwrap()[which]
        .entries
        .last()
        .unwrap()
        .clone()
}

#[tokio::test]
async fn react_a_reply_without_tool_calls_ends_the_run_with_done() {
    let e = ScriptedEngine::new(vec![reply("hi")]);
    let (out, rows) = drive(&e).await;
    assert!(matches!(out, LoopOutcome::Done { ref answer } if answer.text == "hi"));
    assert_eq!(rows.last().unwrap().0, "done");
    assert_eq!(count(&rows, "done"), 1);
    assert_eq!(count(&rows, "error"), 0);
    assert_eq!(count(&rows, "react_turn"), 1);
}

#[tokio::test]
async fn react_tool_results_go_back_to_the_model_in_call_order() {
    let e = ScriptedEngine::new(vec![
        call_tools(vec![call("c1", "echo", json!({"text": "a"}))]),
        reply("ok"),
    ]);
    let (out, rows) = drive(&e).await;
    assert!(matches!(out, LoopOutcome::Done { .. }));
    assert_eq!(
        last_result(&e, 1),
        Entry::ToolResult {
            call_id: "c1".into(),
            text: "a".into(),
            is_error: false
        }
    );
    assert_eq!(count(&rows, "react_tool_result"), 1);
    assert_eq!(count(&rows, "tool_call"), 1);
    assert_eq!(count(&rows, "tool_result"), 1);
}

#[tokio::test]
async fn react_an_unknown_tool_is_a_tool_failure_not_a_run_error() {
    let e = ScriptedEngine::new(vec![
        call_tools(vec![call("c1", "nope", json!({}))]),
        reply("ok"),
    ]);
    let (out, _) = drive(&e).await;
    assert!(matches!(out, LoopOutcome::Done { .. }));
    match last_result(&e, 1) {
        Entry::ToolResult { text, is_error, .. } => {
            assert!(is_error);
            assert!(text.contains("echo"), "{text}");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn react_a_failing_tool_is_reported_to_the_model_and_the_run_continues() {
    let e = ScriptedEngine::new(vec![
        call_tools(vec![call("c1", "fail", json!({}))]),
        reply("ok"),
    ]);
    let (out, _) = drive(&e).await;
    assert!(matches!(out, LoopOutcome::Done { .. }));
    match last_result(&e, 1) {
        Entry::ToolResult { text, is_error, .. } => {
            assert!(is_error);
            assert!(
                text.contains("kaput") && text.contains("try again"),
                "{text}"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn react_an_engine_fatal_error_fails_the_run_with_one_error_row() {
    let e = ScriptedEngine::new(vec![Err(EngineError::Fatal("boom".into()))]);
    let (out, rows) = drive(&e).await;
    assert!(matches!(out, LoopOutcome::Failed { ref message } if message.contains("boom")));
    let (t, p) = rows.last().unwrap();
    assert_eq!(t, "error");
    assert!(p["message"].as_str().unwrap().contains("boom"));
}

#[tokio::test]
async fn react_every_ending_but_suspend_and_cancel_emits_exactly_one_terminal_row() {
    let cases: Vec<(Vec<Result<_, EngineError>>, usize, usize)> = vec![
        (vec![reply("x")], 1, 0),
        (vec![Err(EngineError::Fatal("f".into()))], 0, 1),
        (vec![Err(EngineError::Transient("t".into()))], 0, 1),
    ];
    for (script, done, err) in cases {
        let (_, rows) = drive(&ScriptedEngine::new(script)).await;
        assert_eq!((count(&rows, "done"), count(&rows, "error")), (done, err));
    }
}

#[tokio::test]
async fn react_a_suspend_tool_returns_suspended_with_no_result_and_no_terminal_row() {
    let e = ScriptedEngine::new(vec![call_tools(vec![call(
        "c1",
        "ask_user",
        json!({"question": "which?"}),
    )])]);
    let (out, rows) = drive(&e).await;
    match out {
        LoopOutcome::Suspended {
            call_id,
            question,
            suggestions,
        } => {
            assert_eq!(call_id, "c1");
            assert_eq!(question, "which?");
            assert_eq!(suggestions, vec!["yes", "no"]);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(count(&rows, "react_suspended"), 1);
    assert_eq!(count(&rows, "react_tool_result"), 0);
    assert_eq!(count(&rows, "done") + count(&rows, "error"), 0);
    let (t, p) = rows.iter().find(|(t, _)| t == "react_suspended").unwrap();
    assert_eq!(
        ReactEvent::from_row(t, p),
        Some(ReactEvent::ReactSuspended {
            call_id: "c1".into()
        })
    );
}
