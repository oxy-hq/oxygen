use std::sync::Arc;

use agentic_core::react::{DoneHook, EngineError, EventSink, Toolset, Turn};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::super::testkit::*;
use super::super::{InMemoryStore, LoopConfig, LoopCx, LoopOutcome, ResumeInput, resume_loop};
use super::resume_support::*;

/// Emits an `x` row and upper-cases the answer.
struct UpperHook;

#[async_trait::async_trait]
impl DoneHook for UpperHook {
    async fn on_done(&self, answer: &mut Turn, ev: &EventSink) {
        let _ = ev.send(("x".into(), json!({}))).await;
        answer.text = answer.text.to_uppercase();
    }
}

fn hooked_cfg() -> LoopConfig {
    LoopConfig {
        on_done: Some(Arc::new(UpperHook)),
        nudge: None,
        reflect: None,
        ..cfg(30)
    }
}

fn types(rows: &Rows) -> Vec<&str> {
    rows.iter().map(|(t, _)| t.as_str()).collect()
}

async fn run_hooked(e: &ScriptedEngine) -> (LoopOutcome, Rows) {
    let (tx, rx) = mpsc::channel(256);
    let out = super::super::run_loop(
        e,
        &toolset(vec![]),
        seed(),
        &hooked_cfg(),
        &tx,
        &CancellationToken::new(),
    )
    .await;
    drop(tx);
    (out, collect(rx).await)
}

#[tokio::test]
async fn done_hook_rows_precede_done() {
    let e = ScriptedEngine::new(vec![reply("hi")]);
    let (out, rows) = run_hooked(&e).await;
    let t = types(&rows);
    assert_eq!(&t[t.len() - 2..], ["x", "done"]);
    assert!(matches!(out, LoopOutcome::Done { ref answer } if answer.text == "HI"));
}

#[tokio::test]
async fn done_hook_is_not_called_on_failure() {
    let e = ScriptedEngine::new(vec![Err(EngineError::Fatal("boom".into()))]);
    let (out, rows) = run_hooked(&e).await;
    assert!(matches!(out, LoopOutcome::Failed { .. }));
    assert_eq!(count(&rows, "x"), 0);
    assert_eq!(count(&rows, "error"), 1);
}

#[tokio::test]
async fn resumed_recorded_reply_runs_the_done_hook() {
    let rows = crash_rows(vec![], &[]);
    let rows = rows
        .into_iter()
        .map(|(t, mut p)| {
            if t == "react_turn" {
                p["turn"]["text"] = json!("hi");
            }
            (t, p)
        })
        .collect::<Vec<_>>();
    let e = ScriptedEngine::new(vec![]);
    let (tx, rx) = mpsc::channel(256);
    let cancel = CancellationToken::new();
    let cfg = hooked_cfg();
    let cx = LoopCx {
        cfg: &cfg,
        ev: &tx,
        cancel: &cancel,
        nudge: None,
    };
    let input = ResumeInput {
        run_id: "run1".into(),
        answer: None,
        call_id: None,
    };
    let ts: Toolset = toolset(vec![]);
    let out = resume_loop(&e, &ts, &InMemoryStore::new(rows), seed(), input, cx).await;
    drop(tx);
    let emitted = collect(rx).await;
    let t = types(&emitted);
    assert_eq!(&t[t.len() - 2..], ["x", "done"]);
    assert!(matches!(out, LoopOutcome::Done { ref answer } if answer.text == "HI"));
}
