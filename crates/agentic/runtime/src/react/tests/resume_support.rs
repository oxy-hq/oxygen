//! Shared helpers for the resume tests.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use agentic_core::react::{Effect, Entry, Tool, ToolCallReq, Toolset, Transcript, Turn};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::super::testkit::*;
use super::super::{
    Budgets, InMemoryStore, LoopConfig, LoopCx, LoopOutcome, ReactEvent, ResumeInput, resume_loop,
    run_loop,
};

pub(super) type Rows = Vec<(String, Value)>;
pub(super) type Counter = Arc<AtomicUsize>;

pub(super) fn counted(name: &'static str, effect: Effect) -> (Arc<dyn Tool>, Counter) {
    let count: Counter = Default::default();
    let tool = CountedRead {
        name,
        count: count.clone(),
        effect,
    };
    (Arc::new(tool), count)
}

pub(super) fn toolset(tools: Vec<Arc<dyn Tool>>) -> Toolset {
    Toolset {
        system_prompt: "sys".into(),
        tools,
    }
}

pub(super) fn cfg(max_turns: u32) -> LoopConfig {
    LoopConfig {
        budgets: Budgets {
            max_turns,
            ..Budgets::default()
        },
        run_id: "run1".into(),
        results: Arc::new(NoResults),
        on_done: None,
        nudge: None,
    }
}

pub(super) fn seed() -> Transcript {
    Transcript {
        system: "sys".into(),
        entries: vec![Entry::User("q".into())],
    }
}

pub(super) async fn collect(mut rx: mpsc::Receiver<(String, Value)>) -> Rows {
    let mut rows = Vec::new();
    while let Some(r) = rx.recv().await {
        rows.push(r);
    }
    rows
}

pub(super) async fn first_run(ts: &Toolset, e: &ScriptedEngine) -> (LoopOutcome, Rows) {
    let (tx, rx) = mpsc::channel(256);
    let cancel = CancellationToken::new();
    let out = run_loop(e, ts, seed(), &cfg(30), &tx, &cancel).await;
    drop(tx);
    (out, collect(rx).await)
}

pub(super) async fn resume(
    ts: &Toolset,
    e: &ScriptedEngine,
    rows: Rows,
    answer: Option<&str>,
    max_turns: u32,
) -> (LoopOutcome, Rows) {
    let input = ResumeInput {
        run_id: "run1".into(),
        answer: answer.map(String::from),
        call_id: None,
    };
    resume_with(ts, e, rows, input, max_turns).await
}

pub(super) async fn resume_with(
    ts: &Toolset,
    e: &ScriptedEngine,
    rows: Rows,
    input: ResumeInput,
    max_turns: u32,
) -> (LoopOutcome, Rows) {
    resume_cfg(ts, e, rows, input, cfg(max_turns)).await
}

pub(super) async fn resume_cfg(
    ts: &Toolset,
    e: &ScriptedEngine,
    rows: Rows,
    input: ResumeInput,
    cfg: LoopConfig,
) -> (LoopOutcome, Rows) {
    let (tx, rx) = mpsc::channel(256);
    let cancel = CancellationToken::new();
    let cx = LoopCx {
        cfg: &cfg,
        ev: &tx,
        cancel: &cancel,
        nudge: None,
    };
    let store = InMemoryStore::new(rows);
    let out = resume_loop(e, ts, &store, seed(), input, cx).await;
    drop(tx);
    (out, collect(rx).await)
}

pub(super) fn count(rows: &Rows, t: &str) -> usize {
    rows.iter().filter(|(e, _)| e == t).count()
}

pub(super) fn results_seen(e: &ScriptedEngine) -> Vec<(String, String)> {
    let seen = e.seen.lock().unwrap();
    seen[0]
        .entries
        .iter()
        .filter_map(|x| match x {
            Entry::ToolResult { call_id, text, .. } => Some((call_id.clone(), text.clone())),
            _ => None,
        })
        .collect()
}

pub(super) fn ask(id: &str) -> ToolCallReq {
    call(
        id,
        "ask_user",
        json!({"question": "which quarter?", "suggestions": ["yes", "no"]}),
    )
}

pub(super) fn crash_rows(calls: Vec<ToolCallReq>, recorded: &[&str]) -> Rows {
    let t: Turn = turn("", calls);
    let mut rows = vec![ReactEvent::ReactTurn { turn: t }.to_row()];
    for id in recorded {
        rows.push(
            ReactEvent::ReactToolResult {
                call_id: (*id).into(),
                name: "read_a".into(),
                text: "old".into(),
                is_error: false,
                value: Value::Null,
            }
            .to_row(),
        );
    }
    rows
}
