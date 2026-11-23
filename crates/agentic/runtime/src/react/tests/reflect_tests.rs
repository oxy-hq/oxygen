//! The memory reflection turn: when it runs, what it sees, and that it never
//! changes the answer.

use std::sync::Arc;

use agentic_core::react::{EngineError, Entry, Tool, Toolset, Transcript};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::super::memory::tests::VecStore;
use super::super::testkit::*;
use super::super::{
    AskUser, Budgets, LoopConfig, LoopOutcome, MEMORY_TOOLS, MemoryGate, NoteOp, REFLECT_STATE,
    Reflect, memory_tools, run_loop,
};

/// The run's tools, with the memory tools writing to `store`, and a config
/// whose reflection holds their gate.
fn setup(store: Arc<VecStore>) -> (Toolset, LoopConfig) {
    let gate = MemoryGate::default();
    let mut tools: Vec<Arc<dyn Tool>> =
        vec![Arc::new(EchoRead), Arc::new(FailingRead), Arc::new(AskUser)];
    tools.extend(memory_tools(store, &[], &gate));
    let toolset = Toolset {
        system_prompt: "sys".into(),
        tools,
    };
    let cfg = LoopConfig {
        budgets: Budgets::default(),
        run_id: "run1".into(),
        results: Arc::new(NoResults),
        on_done: None,
        nudge: None,
        reflect: Some(Reflect {
            only: MEMORY_TOOLS.map(String::from).to_vec(),
            query_tool: "echo".into(),
            after_calls: 3,
            prompt: "REFLECT?".into(),
            gate,
        }),
    };
    (toolset, cfg)
}

async fn drive(
    e: &ScriptedEngine,
    run: &(Toolset, LoopConfig),
) -> (LoopOutcome, Vec<(String, Value)>) {
    drive_from(e, run, vec![Entry::User("q".into())]).await
}

async fn drive_from(
    e: &ScriptedEngine,
    (toolset, cfg): &(Toolset, LoopConfig),
    entries: Vec<Entry>,
) -> (LoopOutcome, Vec<(String, Value)>) {
    let (tx, mut rx) = mpsc::channel(256);
    let t = Transcript {
        system: "sys".into(),
        entries,
    };
    let out = run_loop(e, toolset, t, cfg, &tx, &CancellationToken::new()).await;
    drop(tx);
    let mut rows = Vec::new();
    while let Some(r) = rx.recv().await {
        rows.push(r);
    }
    (out, rows)
}

fn echoes(n: usize) -> Vec<Result<agentic_core::react::Decision, EngineError>> {
    (0..n)
        .map(|i| call_tools(vec![call(&format!("c{i}"), "echo", json!({"text": "x"}))]))
        .collect()
}

fn answer_text(out: &LoopOutcome) -> &str {
    match out {
        LoopOutcome::Done { answer } => &answer.text,
        other => panic!("not done: {other:?}"),
    }
}

#[tokio::test]
async fn three_queries_get_a_reflection_turn_that_runs_only_memory_calls() {
    let store = Arc::new(VecStore::default());
    let mut script = echoes(3);
    script.push(reply("the answer"));
    let remember = call("m1", "remember", json!({"text": "echo answers it"}));
    let stray = call("m2", "echo", json!({"text": "not run"}));
    script.push(call_tools(vec![stray, remember]));
    let e = ScriptedEngine::new(script);
    let (out, rows) = drive(&e, &setup(store.clone())).await;

    assert_eq!(answer_text(&out), "the answer");
    let tools = e.seen_tools.lock().unwrap().clone();
    assert_eq!(tools.len(), 5);
    assert_eq!(
        tools[4], tools[0],
        "same tools as the run, so the prompt cache holds"
    );
    let results: Vec<&str> = (rows.iter())
        .filter(|(t, _)| t == "react_tool_result")
        .filter_map(|(_, p)| p["name"].as_str())
        .collect();
    assert_eq!(results, vec!["echo", "echo", "echo", "remember"]);
    let last = e.seen.lock().unwrap()[4].entries.clone();
    assert!(matches!(&last[last.len() - 2], Entry::Assistant(t) if t.text == "the answer"));
    assert!(matches!(&last[last.len() - 1], Entry::User(p) if p == "REFLECT?"));

    let facts = store.0.lock().unwrap().clone();
    assert_eq!(facts.len(), 1);
    assert_eq!(
        (facts[0].op, facts[0].text.as_str()),
        (NoteOp::Put, "echo answers it")
    );
    let states: Vec<&str> = (rows.iter())
        .filter(|(t, _)| t == "llm_end")
        .filter_map(|(_, p)| p["state"].as_str())
        .collect();
    assert_eq!(states.last(), Some(&REFLECT_STATE));
    let marks: Vec<&str> = (rows.iter())
        .filter(|(t, p)| (t == "state_enter" || t == "state_exit") && p["state"] == REFLECT_STATE)
        .map(|(t, _)| t.as_str())
        .collect();
    assert_eq!(marks, vec!["state_enter", "state_exit"]);
    assert_eq!(rows.last().unwrap().0, "done", "done stays the last row");
}

#[tokio::test]
async fn two_clean_queries_skip_the_reflection_turn() {
    let store = Arc::new(VecStore::default());
    let mut script = echoes(2);
    script.push(reply("easy"));
    // Scripted for exactly three calls: a fourth would panic.
    let e = ScriptedEngine::new(script);
    let (out, _) = drive(&e, &setup(store.clone())).await;
    assert_eq!(answer_text(&out), "easy");
    assert!(store.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_failed_query_triggers_it_early() {
    let store = Arc::new(VecStore::default());
    let mut run = setup(store.clone());
    if let Some(r) = run.1.reflect.as_mut() {
        r.query_tool = "fail".into();
    }
    let e = ScriptedEngine::new(vec![
        call_tools(vec![call("c0", "fail", json!({}))]),
        reply("partial"),
        reply("no"),
    ]);
    let (out, _) = drive(&e, &run).await;
    assert_eq!(answer_text(&out), "partial");
    assert_eq!(e.seen_tools.lock().unwrap().len(), 3);
    assert!(
        store.0.lock().unwrap().is_empty(),
        "a \"no\" writes nothing"
    );
}

#[tokio::test]
async fn a_reflection_error_still_ends_done_with_the_answer() {
    let store = Arc::new(VecStore::default());
    let mut script = echoes(3);
    script.push(reply("kept"));
    script.push(Err(EngineError::Fatal("boom".into())));
    let e = ScriptedEngine::new(script);
    let (out, rows) = drive(&e, &setup(store)).await;
    assert_eq!(answer_text(&out), "kept");
    assert_eq!(rows.last().unwrap().0, "done");
    assert!(!rows.iter().any(|(t, _)| t == "error"));
}

#[tokio::test]
async fn no_reflect_config_means_no_extra_turn() {
    let mut script = echoes(5);
    script.push(reply("done"));
    let e = ScriptedEngine::new(script);
    let mut run = setup(Arc::new(VecStore::default()));
    run.1.reflect = None;
    let (out, _) = drive(&e, &run).await;
    assert_eq!(answer_text(&out), "done");
    assert_eq!(e.seen_tools.lock().unwrap().len(), 6);
}

#[tokio::test]
async fn an_answered_ask_user_triggers_it() {
    let store = Arc::new(VecStore::default());
    let ask = call("a1", "ask_user", json!({"question": "which year?"}));
    let entries = vec![
        Entry::User("q".into()),
        Entry::Assistant(turn("", vec![ask])),
        Entry::ToolResult {
            call_id: "a1".into(),
            text: "2020".into(),
            is_error: false,
        },
    ];
    let e = ScriptedEngine::new(vec![reply("for 2020"), reply("no")]);
    let (out, _) = drive_from(&e, &setup(store), entries).await;
    assert_eq!(answer_text(&out), "for 2020");
    assert_eq!(e.seen_tools.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_remember_call_while_answering_is_refused_and_saves_nothing() {
    let store = Arc::new(VecStore::default());
    let early = call("m0", "remember", json!({"text": "too soon"}));
    let e = ScriptedEngine::new(vec![call_tools(vec![early]), reply("answer")]);
    let (out, rows) = drive(&e, &setup(store.clone())).await;
    assert_eq!(answer_text(&out), "answer");
    assert!(store.0.lock().unwrap().is_empty());
    let refused = (rows.iter())
        .find(|(t, p)| t == "react_tool_result" && p["name"] == "remember")
        .map(|(_, p)| p.clone())
        .unwrap();
    assert_eq!(refused["is_error"], true);
}
