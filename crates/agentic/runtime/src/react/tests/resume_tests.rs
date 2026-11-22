use std::sync::Arc;
use std::sync::atomic::Ordering;

use agentic_core::human_input::SuspendedRunData;
use agentic_core::react::{Decision, Effect};
use serde_json::json;

use super::super::AskUser;
use super::super::testkit::*;
use super::super::{LoopOutcome, Nudge, ResumeInput, SuspensionInput, suspension_data};
use super::resume_support::*;

#[tokio::test]
async fn resume_injects_the_answer_on_the_suspended_call() {
    let ts = toolset(vec![Arc::new(AskUser)]);
    let e1 = ScriptedEngine::new(vec![call_tools(vec![ask("c1")])]);
    let (out, rows) = first_run(&ts, &e1).await;
    assert!(matches!(out, LoopOutcome::Suspended { ref call_id, .. } if call_id == "c1"));
    let e2 = ScriptedEngine::new(vec![reply("done")]);
    let (out, new_rows) = resume(&ts, &e2, rows, Some("Q3 2025"), 30).await;
    assert!(matches!(out, LoopOutcome::Done { .. }));
    assert_eq!(results_seen(&e2), vec![("c1".into(), "Q3 2025".into())]);
    assert_eq!(count(&new_rows, "done"), 1);
    assert_eq!(count(&new_rows, "error"), 0);
    assert_eq!(count(&new_rows, "react_suspended"), 0);
}

#[tokio::test]
async fn resume_never_reruns_a_call_with_a_recorded_result() {
    let (a, ca) = counted("read_a", Effect::Read);
    let (b, cb) = counted("read_b", Effect::Read);
    let ts = toolset(vec![a, b, Arc::new(AskUser)]);
    let e1 = ScriptedEngine::new(vec![call_tools(vec![
        call("c1", "read_a", json!({})),
        call("c2", "read_b", json!({})),
        ask("c3"),
    ])]);
    let (_, rows) = first_run(&ts, &e1).await;
    let e2 = ScriptedEngine::new(vec![reply("done")]);
    let (out, _) = resume(&ts, &e2, rows, Some("Q3"), 30).await;
    assert!(matches!(out, LoopOutcome::Done { .. }));
    assert_eq!(
        (ca.load(Ordering::SeqCst), cb.load(Ordering::SeqCst)),
        (1, 1)
    );
    let ids: Vec<String> = results_seen(&e2).into_iter().map(|r| r.0).collect();
    assert_eq!(ids, vec!["c1", "c2", "c3"]);
}

#[tokio::test]
async fn resume_runs_calls_after_the_suspended_one_in_call_order() {
    let (a, ca) = counted("read_a", Effect::Read);
    let (c, cc) = counted("read_c", Effect::Read);
    let ts = toolset(vec![a, c, Arc::new(AskUser)]);
    let e1 = ScriptedEngine::new(vec![call_tools(vec![
        call("c1", "read_a", json!({})),
        ask("c2"),
        call("c3", "read_c", json!({})),
    ])]);
    let (_, rows) = first_run(&ts, &e1).await;
    assert_eq!(cc.load(Ordering::SeqCst), 0);
    let e2 = ScriptedEngine::new(vec![reply("done")]);
    let (_, new_rows) = resume(&ts, &e2, rows, Some("Q3"), 30).await;
    assert_eq!(
        (ca.load(Ordering::SeqCst), cc.load(Ordering::SeqCst)),
        (1, 1)
    );
    let seen = results_seen(&e2);
    let ids: Vec<&str> = seen.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(ids, vec!["c1", "c2", "c3"]);
    assert_eq!(seen[1].1, "Q3");
    assert_eq!(count(&new_rows, "react_tool_result"), 2);
}

#[tokio::test]
async fn crash_recovery_runs_only_unrecorded_calls() {
    let (a, ca) = counted("read_a", Effect::Read);
    let (b, cb) = counted("read_b", Effect::Read);
    let ts = toolset(vec![a, b]);
    let calls = vec![
        call("c1", "read_a", json!({})),
        call("c2", "read_b", json!({})),
    ];
    let e = ScriptedEngine::new(vec![reply("done")]);
    let (out, _) = resume(&ts, &e, crash_rows(calls, &["c1"]), None, 30).await;
    assert!(matches!(out, LoopOutcome::Done { .. }));
    assert_eq!(
        (ca.load(Ordering::SeqCst), cb.load(Ordering::SeqCst)),
        (0, 1)
    );
    let seen = results_seen(&e);
    assert_eq!(seen[0], ("c1".into(), "old".into()));
    assert_eq!(seen[1].0, "c2");
    assert_eq!(seen.len(), 2);
}

#[tokio::test]
async fn a_pending_write_is_not_rerun_after_a_crash() {
    let (w, cw) = counted("write_a", Effect::Write);
    let ts = toolset(vec![w]);
    let calls = vec![call("c1", "write_a", json!({}))];
    let e = ScriptedEngine::new(vec![]);
    let (out, rows) = resume(&ts, &e, crash_rows(calls, &[]), None, 30).await;
    let LoopOutcome::Failed { message } = out else {
        panic!("expected Failed");
    };
    assert_eq!(message, "cannot re-run a write call after a crash");
    assert_eq!(cw.load(Ordering::SeqCst), 0);
    assert_eq!((count(&rows, "error"), count(&rows, "done")), (1, 0));
}

#[tokio::test]
async fn a_write_after_the_parked_ask_runs_once_on_resume() {
    let (w, cw) = counted("write_a", Effect::Write);
    let ts = toolset(vec![Arc::new(AskUser), w]);
    let e1 = ScriptedEngine::new(vec![call_tools(vec![
        ask("c1"),
        call("c2", "write_a", json!({})),
    ])]);
    let (out, rows) = first_run(&ts, &e1).await;
    assert!(matches!(out, LoopOutcome::Suspended { ref call_id, .. } if call_id == "c1"));
    assert_eq!(cw.load(Ordering::SeqCst), 0);
    let e2 = ScriptedEngine::new(vec![reply("done")]);
    let (out, new_rows) = resume(&ts, &e2, rows, Some("A"), 30).await;
    assert!(matches!(out, LoopOutcome::Done { .. }));
    assert_eq!(cw.load(Ordering::SeqCst), 1);
    assert_eq!(
        (count(&new_rows, "done"), count(&new_rows, "error")),
        (1, 0)
    );
}

#[tokio::test]
async fn crash_recovery_refuses_a_write_behind_an_unrecorded_read() {
    // The read's result may have been queued but never flushed, so the log
    // cannot prove the write after it never started.
    let (r, cr) = counted("read_a", Effect::Read);
    let (w, cw) = counted("write_a", Effect::Write);
    let ts = toolset(vec![r, w]);
    let calls = vec![
        call("c1", "read_a", json!({})),
        call("c2", "write_a", json!({})),
    ];
    let e = ScriptedEngine::new(vec![]);
    let (out, rows) = resume(&ts, &e, crash_rows(calls, &[]), None, 30).await;
    let LoopOutcome::Failed { message } = out else {
        panic!("expected Failed");
    };
    assert_eq!(message, "cannot re-run a write call after a crash");
    assert_eq!(
        (cr.load(Ordering::SeqCst), cw.load(Ordering::SeqCst)),
        (0, 0)
    );
    assert_eq!((count(&rows, "done"), count(&rows, "error")), (0, 1));
}

#[tokio::test]
async fn resume_budget_counts_turns_already_taken() {
    let ts = toolset(vec![Arc::new(EchoRead)]);
    let calls = vec![call("c1", "echo", json!({"text": "x"}))];
    let rows = crash_rows(calls, &["c1"]);
    let e = ScriptedEngine::new(vec![
        call_tools(vec![call("c2", "echo", json!({"text": "y"}))]),
        reply("never reached"),
    ]);
    let (out, _) = resume(&ts, &e, rows, None, 2).await;
    let LoopOutcome::Failed { message } = out else {
        panic!("expected Failed");
    };
    assert_eq!(message, "budget exceeded: turns 2/2");
    assert_eq!(e.seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_second_suspend_after_resume_emits_only_the_pointer_row() {
    let ts = toolset(vec![Arc::new(AskUser)]);
    let e1 = ScriptedEngine::new(vec![call_tools(vec![ask("c1")])]);
    let (_, rows) = first_run(&ts, &e1).await;
    let e2 = ScriptedEngine::new(vec![Ok(Decision::CallTools(turn("", vec![ask("c2")])))]);
    let (out, new_rows) = resume(&ts, &e2, rows, Some("A"), 30).await;
    assert!(matches!(out, LoopOutcome::Suspended { ref call_id, .. } if call_id == "c2"));
    assert_eq!(count(&new_rows, "react_suspended"), 1);
    assert_eq!(count(&new_rows, "done") + count(&new_rows, "error"), 0);
}

#[test]
fn suspension_data_round_trips_through_its_serde() {
    let suggestions = vec!["yes".to_string(), "no".to_string()];
    let d = suspension_data(SuspensionInput {
        run_id: "run1",
        call_id: "c1",
        question: "which quarter?",
        suggestions: &suggestions,
        original_input: "revenue?",
        trace_id: "t1",
    });
    assert_eq!(d.from_state, "react");
    assert_eq!(d.stage_data, json!({"run_id": "run1", "call_id": "c1"}));
    let back: SuspendedRunData = serde_json::from_value(serde_json::to_value(&d).unwrap()).unwrap();
    assert_eq!(back.question, "which quarter?");
    assert_eq!(back.suggestions, suggestions);
    assert_eq!(back.stage_data, d.stage_data);
    assert_eq!(
        (back.original_input.as_str(), back.trace_id.as_str()),
        ("revenue?", "t1")
    );
}

#[tokio::test]
async fn artifacts_are_not_reemitted_on_resume() {
    let ts = toolset(vec![Arc::new(ArtifactRead), Arc::new(AskUser)]);
    let e1 = ScriptedEngine::new(vec![call_tools(vec![
        call("c1", "artifact_read", json!({})),
        ask("c2"),
    ])]);
    let (out, rows) = first_run(&ts, &e1).await;
    assert!(matches!(out, LoopOutcome::Suspended { .. }));
    assert_eq!(count(&rows, "query_executed"), 1);
    let e2 = ScriptedEngine::new(vec![reply("done")]);
    let (out, new_rows) = resume(&ts, &e2, rows, Some("A"), 30).await;
    assert!(matches!(out, LoopOutcome::Done { .. }));
    assert_eq!(count(&new_rows, "query_executed"), 0);
}

/// Calls run during a resume count towards the nudge, and its note is saved
/// on the result that met the trigger.
#[tokio::test]
async fn crash_recovery_counts_pending_queries_towards_the_nudge() {
    let (q, _) = counted("query", Effect::Read);
    let ts = toolset(vec![q]);
    let calls = vec![
        call("c1", "query", json!({})),
        call("c2", "query", json!({})),
        call("c3", "query", json!({})),
    ];
    let mut cfg = cfg(30);
    cfg.nudge = Some(Nudge {
        tool: "query".into(),
        after_calls: 3,
        text: "NUDGE".into(),
    });
    let input = ResumeInput {
        run_id: "run1".into(),
        answer: None,
        call_id: None,
    };
    let e = ScriptedEngine::new(vec![reply("done")]);
    let (out, rows) = resume_cfg(&ts, &e, crash_rows(calls, &["c1"]), input, cfg).await;
    assert!(matches!(out, LoopOutcome::Done { .. }));
    let saved: Vec<(&str, &str)> = rows
        .iter()
        .filter(|(t, _)| t == "react_tool_result")
        .map(|(_, v)| (v["call_id"].as_str().unwrap(), v["text"].as_str().unwrap()))
        .collect();
    assert_eq!(saved.len(), 2, "{rows:?}");
    assert!(!saved[0].1.contains("NUDGE"), "{saved:?}");
    assert_eq!(saved[1].0, "c3");
    assert!(saved[1].1.ends_with("\n\nNUDGE"), "{saved:?}");
}
