//! What routes a resume to react, what it hands the runtime, and the one
//! retry a refused answer gets. The served resume end to end, against
//! Postgres, is `tests/integration/react_resume.rs`.

use agentic_core::react::{Decision, ToolCallReq};
use agentic_runtime::react::{InMemoryStore, ReactEvent};
use serde_json::json;

use super::*;
use crate::react_run::tests::{react_run, turn};

fn run_row(metadata: Option<Value>) -> agentic_runtime::entity::run::Model {
    let now = agentic_runtime::crud::now();
    agentic_runtime::entity::run::Model {
        id: "run-1".into(),
        question: "which year sold most?".into(),
        answer: None,
        error_message: None,
        thread_id: None,
        source_type: Some("analytics".into()),
        metadata,
        parent_run_id: None,
        schedule_id: None,
        task_status: Some("running".into()),
        task_metadata: None,
        attempt: 0,
        recovery_requested_at: None,
        driver_id: None,
        driver_heartbeat_at: None,
        cancel_requested_at: None,
        workspace_id: uuid::Uuid::nil(),
        created_at: now,
        updated_at: now,
    }
}

fn suspended(call_id: Option<&str>) -> SuspendedRunData {
    let mut stage_data = json!({ "run_id": "run-1" });
    if let Some(id) = call_id {
        stage_data["call_id"] = json!(id);
    }
    SuspendedRunData {
        from_state: "react".into(),
        original_input: "q".into(),
        trace_id: "run-1".into(),
        stage_data,
        question: "Which year?".into(),
        suggestions: vec![],
    }
}

/// Crash recovery passes `""`; react reads a blank answer as none. The call
/// id is the suspension's.
#[test]
fn react_resume_reads_a_blank_answer_as_none() {
    let data = suspended(Some("c1"));
    for blank in ["", "   ", "\n\t"] {
        let input = resume_input("run-1", &data, blank);
        assert_eq!(input.answer, None, "{blank:?}");
    }
    let input = resume_input("run-1", &data, "2024");
    assert_eq!(input.answer.as_deref(), Some("2024"));
    assert_eq!(input.call_id.as_deref(), Some("c1"));
    assert_eq!(input.run_id, "run-1");
    assert_eq!(resume_input("run-1", &suspended(None), "x").call_id, None);
}

/// A run with no suspension is recovered on react only when its metadata
/// says its latest attempt ran react; `fsm`, or no key, stays on the FSM.
#[test]
fn react_resume_recovery_routes_on_the_run_metadata_only() {
    let react = run_row(Some(json!({ "agent_id": "a", "runtime": "react" })));
    let data = recovery_data(&react).expect("a react run recovers on react");
    assert!(is_react(&data));
    assert_eq!(data.stage_data, json!({ "run_id": "run-1" }), "no call id");
    assert_eq!(data.original_input, "which year sold most?");
    let input = resume_input("run-1", &data, "");
    assert!(input.answer.is_none() && input.call_id.is_none());

    let fsm = run_row(Some(json!({ "agent_id": "a", "runtime": "fsm" })));
    assert!(recovery_data(&fsm).is_none());
    assert!(recovery_data(&run_row(Some(json!({ "agent_id": "a" })))).is_none());
    assert!(recovery_data(&run_row(None)).is_none());

    let mut clarifying = suspended(Some("c1"));
    clarifying.from_state = "clarifying".into();
    assert!(!is_react(&clarifying));
}

fn ask(id: &str) -> ToolCallReq {
    ToolCallReq {
        id: id.into(),
        name: "ask_user".into(),
        input: json!({ "question": "Which year?" }),
    }
}

async fn resume_on(
    rows: Vec<(String, Value)>,
    script: Vec<Decision>,
    input: ResumeInput,
) -> LoopOutcome {
    let dir = tempfile::tempdir().unwrap();
    let run = react_run(script, dir.path());
    let (ev, mut sink) = tokio::sync::mpsc::channel(256);
    let drain = tokio::spawn(async move { while sink.recv().await.is_some() {} });
    let seed = Seed {
        prior: vec![],
        question: "q".into(),
    };
    let store = InMemoryStore::new(rows);
    let outcome = resume_react(run, &store, seed, input, ev, CancellationToken::new()).await;
    drain.await.unwrap();
    outcome
}

/// An answer for a call the run is not parked on is refused by the runtime;
/// the served resume retries once with no answer, so the parked question is
/// asked again rather than the run failing.
#[tokio::test]
async fn react_resume_retries_a_refused_answer_without_it() {
    let rows = vec![
        ReactEvent::ReactTurn {
            turn: turn("", vec![ask("c1")]),
        }
        .to_row(),
        ReactEvent::ReactSuspended {
            call_id: "c1".into(),
        }
        .to_row(),
    ];
    let input = ResumeInput {
        run_id: "run-1".into(),
        answer: Some("2024".into()),
        call_id: Some("c9".into()),
    };
    let outcome = resume_on(rows, vec![], input).await;
    assert!(
        matches!(outcome, LoopOutcome::Suspended { ref call_id, .. } if call_id == "c1"),
        "the question is asked again: {outcome:?}"
    );
}

/// An answer with nothing parked (the call already has its result) is
/// refused, then the run continues without it: the model is asked next.
#[tokio::test]
async fn react_resume_with_nothing_parked_continues_without_the_answer() {
    let rows = vec![
        ReactEvent::ReactTurn {
            turn: turn("", vec![ask("c1")]),
        }
        .to_row(),
        ReactEvent::ReactToolResult {
            call_id: "c1".into(),
            name: "ask_user".into(),
            text: "2023".into(),
            is_error: false,
            value: Value::Null,
        }
        .to_row(),
    ];
    let input = ResumeInput {
        run_id: "run-1".into(),
        answer: Some("2024".into()),
        call_id: None,
    };
    let script = vec![Decision::Reply(turn("2023 it is", vec![]))];
    let outcome = resume_on(rows, script, input).await;
    let LoopOutcome::Done { answer } = outcome else {
        panic!("expected Done, got {outcome:?}");
    };
    assert_eq!(answer.text, "2023 it is");
}

/// A resume rebuilds at the thinking mode the run started with: an
/// extended-thinking chat resumes on the agent's `llm.extended_thinking`
/// model, so its recorded thinking blocks replay into the model that wrote
/// them.
#[test]
fn react_resume_keeps_the_started_thinking_mode() {
    let extended = json!({ "agent_id": "a", "thinking_mode": "extended_thinking" });
    assert_eq!(
        recorded_thinking_mode(Some(&extended)),
        ThinkingMode::ExtendedThinking
    );
    let auto = json!({ "agent_id": "a", "thinking_mode": "auto" });
    assert_eq!(recorded_thinking_mode(Some(&auto)), ThinkingMode::Auto);
    assert_eq!(recorded_thinking_mode(None), ThinkingMode::Auto);

    let config = AgentConfig::from_yaml(
        "runtime: react\nllm:\n  model: base\n  extended_thinking:\n    model: deep\n",
    )
    .unwrap();
    let inputs = ServedInputs {
        agent_id: "a".into(),
        workspace_id: uuid::Uuid::nil(),
        connectors: Default::default(),
        databases: Vec::new(),
        project_model: None,
        domain_docs: None,
        thinking_mode: recorded_thinking_mode(Some(&extended)),
    };
    let ctx = build_context(&crate::react_run::tests::NoPlatform, &config, inputs);
    assert_eq!(ctx.model_override.as_deref(), Some("deep"));
}

/// A start continues the log instead of starting over once it holds an
/// earlier attempt's react rows or an end; a `runtime_selected` alone (a
/// crash before the first turn) starts over.
#[test]
fn react_redrive_continues_only_a_log_with_an_attempt() {
    let turn_row = ReactEvent::ReactTurn {
        turn: turn("", vec![ask("c1")]),
    }
    .to_row();
    let done = ("done".to_string(), json!({ "trace_id": "run-1" }));
    let selected = ("runtime_selected".to_string(), json!({}));
    assert!(continues(&[selected.clone(), turn_row]));
    assert!(continues(&[done]));
    assert!(!continues(&[selected]));
    assert!(!continues(&[]));
}

/// A start stays on react when its log holds a react attempt, and fails
/// closed on react when the log cannot be read; an FSM end alone is no
/// react attempt.
#[test]
fn react_redrive_is_decided_by_react_rows_in_the_log() {
    let turn_row = ReactEvent::ReactTurn {
        turn: turn("", vec![ask("c1")]),
    }
    .to_row();
    let done = ("done".to_string(), json!({ "trace_id": "run-1" }));
    assert!(holds_react(&Ok(vec![turn_row])));
    assert!(holds_react(&Err("db down".into())));
    assert!(!holds_react(&Ok(vec![done])));
    assert!(!holds_react(&Ok(vec![])));
}

/// A failing log reader that counts its calls.
fn failing_reader(
    calls: &std::sync::atomic::AtomicUsize,
) -> impl std::future::Future<Output = PriorLog> + '_ {
    async move {
        calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err("the log store is down".to_string())
    }
}

/// A run never recorded `react` (no key, or `fsm`) reads no log, so a
/// failing log store cannot touch its start; a run recorded `react` reads
/// it and fails closed on react.
#[tokio::test]
async fn react_redrive_reads_the_log_only_of_a_run_recorded_react() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = AtomicUsize::new(0);
    for recorded in [None, Some("fsm")] {
        let log = prior_log(recorded, || failing_reader(&calls)).await;
        assert!(log.is_none(), "{recorded:?}");
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "no read for a never-react run"
    );

    let log = prior_log(Some("react"), || failing_reader(&calls)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        log.as_ref().is_some_and(holds_react),
        "fails closed on react"
    );
}

/// The recorded runtime comes from what the driver read, without a lookup
/// (the connection here is disconnected), and parses the row's key.
#[tokio::test]
async fn react_runtime_of_uses_what_the_driver_read() {
    let db = sea_orm::DatabaseConnection::default();
    let read = |r: Option<&str>| RecordedRuntime::Read(r.map(String::from));
    assert_eq!(
        runtime_of(&db, "run-1", &read(Some("react")))
            .await
            .as_deref(),
        Some("react")
    );
    assert_eq!(runtime_of(&db, "run-1", &read(None)).await, None);
    let recorded = json!({ "agent_id": "a", "runtime": "react" });
    assert_eq!(
        crate::queued_run::recorded_runtime(Some(&recorded)).as_deref(),
        Some("react")
    );
    assert_eq!(crate::queued_run::recorded_runtime(None), None);
}
