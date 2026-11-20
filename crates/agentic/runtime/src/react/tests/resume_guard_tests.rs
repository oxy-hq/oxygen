//! A resume that must not run: finished runs, crashed replies, wrong calls,
//! and cancellation while the pending calls run.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::super::AskUser;
use super::super::testkit::*;
use super::super::{InMemoryStore, LoopCx, LoopOutcome, ReactEvent, ResumeInput, resume_loop};
use super::resume_support::*;

fn input(answer: &str, call_id: Option<&str>) -> ResumeInput {
    ResumeInput {
        run_id: "run1".into(),
        answer: Some(answer.into()),
        call_id: call_id.map(String::from),
    }
}

#[tokio::test]
async fn resuming_a_finished_run_returns_done_and_writes_no_row() {
    let ts = toolset(vec![Arc::new(AskUser)]);
    let e1 = ScriptedEngine::new(vec![call_tools(vec![ask("c1")])]);
    let (_, mut rows) = first_run(&ts, &e1).await;
    let e2 = ScriptedEngine::new(vec![reply("done")]);
    let (_, more) = resume(&ts, &e2, rows.clone(), Some("A"), 30).await;
    rows.extend(more);
    let e3 = ScriptedEngine::new(vec![]);
    let (out, new_rows) = resume(&ts, &e3, rows, Some("B"), 30).await;
    assert!(matches!(out, LoopOutcome::Done { ref answer } if answer.text == "done"));
    assert!(new_rows.is_empty(), "{new_rows:?}");
    assert_eq!(count(&new_rows, "error"), 0);
}

#[tokio::test]
async fn resuming_a_failed_run_returns_its_error_and_writes_no_row() {
    let ts = toolset(vec![Arc::new(AskUser)]);
    let e1 = ScriptedEngine::new(vec![call_tools(vec![ask("c1")])]);
    let (_, mut rows) = first_run(&ts, &e1).await;
    rows.push((
        "error".into(),
        json!({"message": "boom", "trace_id": "run1"}),
    ));
    let e2 = ScriptedEngine::new(vec![]);
    let (out, new_rows) = resume(&ts, &e2, rows, Some("A"), 30).await;
    assert!(matches!(out, LoopOutcome::Failed { ref message } if message == "boom"));
    assert!(new_rows.is_empty(), "{new_rows:?}");
}

#[tokio::test]
async fn a_reply_recorded_before_a_crash_finishes_with_done_only() {
    let ts = toolset(vec![Arc::new(EchoRead)]);
    let rows = vec![
        ReactEvent::ReactTurn {
            turn: turn("final", vec![]),
        }
        .to_row(),
    ];
    // An empty script: the engine panics if it is asked to decide again.
    let e = ScriptedEngine::new(vec![]);
    let (out, new_rows) = resume(&ts, &e, rows, None, 30).await;
    assert!(matches!(out, LoopOutcome::Done { ref answer } if answer.text == "final"));
    let types: Vec<&str> = new_rows.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(types, ["done"]);
    assert!(e.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_answer_with_no_parked_call_is_refused_without_a_row() {
    let ts = toolset(vec![Arc::new(EchoRead)]);
    let rows = crash_rows(vec![call("c1", "echo", json!({}))], &["c1"]);
    let e = ScriptedEngine::new(vec![]);
    let (out, new_rows) = resume(&ts, &e, rows, Some("A"), 30).await;
    assert!(matches!(out, LoopOutcome::Refused { .. }), "{out:?}");
    assert!(new_rows.is_empty(), "{new_rows:?}");
}

#[tokio::test]
async fn a_resume_naming_another_call_is_refused_without_a_row() {
    let ts = toolset(vec![Arc::new(AskUser)]);
    let e1 = ScriptedEngine::new(vec![call_tools(vec![ask("c1")])]);
    let (_, mut rows) = first_run(&ts, &e1).await;
    let e2 = ScriptedEngine::new(vec![call_tools(vec![ask("c2")])]);
    let (out, more) = resume_with(&ts, &e2, rows.clone(), input("A", Some("c1")), 30).await;
    assert!(matches!(out, LoopOutcome::Suspended { ref call_id, .. } if call_id == "c2"));
    rows.extend(more);
    // A duplicate delivery of the first resume, now that the run waits on c2.
    let e3 = ScriptedEngine::new(vec![]);
    let (out, new_rows) = resume_with(&ts, &e3, rows.clone(), input("A", Some("c1")), 30).await;
    let LoopOutcome::Refused { reason } = out else {
        panic!("expected Refused, got {out:?}");
    };
    assert!(reason.contains("c1") && reason.contains("c2"), "{reason}");
    assert!(new_rows.is_empty(), "{new_rows:?}");
    // The right id still lands.
    let e4 = ScriptedEngine::new(vec![reply("ok")]);
    let (out, _) = resume_with(&ts, &e4, rows, input("B", Some("c2")), 30).await;
    assert!(matches!(out, LoopOutcome::Done { .. }));
}

#[tokio::test]
async fn answering_the_first_of_two_asks_suspends_on_the_second() {
    let ts = toolset(vec![Arc::new(AskUser)]);
    let e1 = ScriptedEngine::new(vec![call_tools(vec![ask("c1"), ask("c2")])]);
    let (out, rows) = first_run(&ts, &e1).await;
    assert!(matches!(out, LoopOutcome::Suspended { ref call_id, .. } if call_id == "c1"));
    let e2 = ScriptedEngine::new(vec![]);
    let (out, new_rows) = resume_with(&ts, &e2, rows, input("A", Some("c1")), 30).await;
    assert!(matches!(out, LoopOutcome::Suspended { ref call_id, .. } if call_id == "c2"));
    assert_eq!(count(&new_rows, "react_tool_result"), 1);
    assert_eq!(count(&new_rows, "react_suspended"), 1);
    assert_eq!(count(&new_rows, "done") + count(&new_rows, "error"), 0);
}

#[tokio::test(start_paused = true)]
async fn cancel_while_a_resume_runs_pending_calls_returns_cancelled() {
    let ts = toolset(vec![Arc::new(SlowRead(60_000))]);
    let rows = crash_rows(vec![call("c1", "slow", json!({}))], &[]);
    let e = ScriptedEngine::new(vec![]);
    let (tx, rx) = mpsc::channel(256);
    let cancel = CancellationToken::new();
    let cfg = cfg(30);
    let store = InMemoryStore::new(rows);
    let none = ResumeInput {
        run_id: "run1".into(),
        answer: None,
        call_id: None,
    };
    let cx = LoopCx {
        cfg: &cfg,
        ev: &tx,
        cancel: &cancel,
    };
    let canceller = async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();
    };
    let (out, ()) = tokio::join!(resume_loop(&e, &ts, &store, seed(), none, cx), canceller);
    drop(tx);
    let new_rows = collect(rx).await;
    assert!(matches!(out, LoopOutcome::Cancelled), "{out:?}");
    assert_eq!(count(&new_rows, "react_tool_result"), 0);
    assert_eq!(count(&new_rows, "done") + count(&new_rows, "error"), 0);
}
