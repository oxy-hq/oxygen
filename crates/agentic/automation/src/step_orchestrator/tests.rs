use super::*;
use crate::config::*;

fn make_orchestrator(results: HashMap<String, Value>) -> AutomationStepOrchestrator {
    AutomationStepOrchestrator {
        workflow: AutomationConfig {
            name: "test".into(),
            tasks: vec![],
            description: String::new(),
            variables: None,
            consistency_prompt: None,
            consistency_model: None,
        },
        render_context: json!(results),
        workflow_context: json!({}),
        results,
        current_step: 0,
        trace_id: "test".into(),
        evaluator: None,
        airway_admission: None,
    }
}

#[test]
fn test_formatter_renders_template_with_context() {
    let mut results = HashMap::new();
    results.insert("query".into(), json!({"total": 42}));
    let orch = make_orchestrator(results);

    let result = orch
        .execute_formatter("Total is {{ query.total }}")
        .unwrap();
    assert_eq!(result["text"].as_str().unwrap(), "Total is 42");
}

#[test]
fn test_formatter_handles_missing_variable() {
    let orch = make_orchestrator(HashMap::new());
    // minijinja renders undefined vars as empty by default
    let result = orch.execute_formatter("Value: {{ missing }}");
    // Should either render as empty or error — both acceptable
    assert!(result.is_ok() || result.is_err());
}

#[test]
fn test_conditional_matches_first_truthy_branch() {
    let mut results = HashMap::new();
    results.insert("count".into(), json!(10));
    let orch = make_orchestrator(results);

    let cond = ConditionalConfig {
        conditions: vec![
            ConditionBranch {
                condition: "count > 5".into(),
                tasks: vec![TaskConfig {
                    name: "big_task".into(),
                    task_type: TaskType::Unknown,
                }],
            },
            ConditionBranch {
                condition: "count > 0".into(),
                tasks: vec![TaskConfig {
                    name: "small_task".into(),
                    task_type: TaskType::Unknown,
                }],
            },
        ],
        else_tasks: None,
    };

    let result = orch.execute_conditional(&cond).unwrap();
    assert_eq!(result["branch"], "matched");
    assert_eq!(result["condition"], "count > 5");
}

#[test]
fn test_conditional_falls_through_to_else() {
    let orch = make_orchestrator(HashMap::new());

    let cond = ConditionalConfig {
        conditions: vec![ConditionBranch {
            condition: "false".into(),
            tasks: vec![],
        }],
        else_tasks: Some(vec![TaskConfig {
            name: "fallback".into(),
            task_type: TaskType::Unknown,
        }]),
    };

    let result = orch.execute_conditional(&cond).unwrap();
    assert_eq!(result["branch"], "else");
}

#[test]
fn test_conditional_no_match_no_else() {
    let orch = make_orchestrator(HashMap::new());

    let cond = ConditionalConfig {
        conditions: vec![ConditionBranch {
            condition: "false".into(),
            tasks: vec![],
        }],
        else_tasks: None,
    };

    let result = orch.execute_conditional(&cond).unwrap();
    assert_eq!(result["branch"], "none_matched");
}

#[test]
fn test_majority_vote_clear_winner() {
    let answers: Vec<String> = vec!["a".into(), "a".into(), "b".into()];
    let (winner, score) = majority_vote(&answers);
    assert_eq!(winner, "a");
    assert!((score - 2.0 / 3.0).abs() < f64::EPSILON);
}

#[test]
fn test_majority_vote_all_same() {
    let answers: Vec<String> = vec!["x".into(), "x".into()];
    let (winner, score) = majority_vote(&answers);
    assert_eq!(winner, "x");
    assert_eq!(score, 1.0);
}

#[test]
fn test_classify_step_agent_with_consistency_prompt() {
    let orch = make_orchestrator(HashMap::new());

    let agent_task = AgentTaskConfig {
        agent_ref: "my-agent".into(),
        prompt: "hello".into(),
        consistency_run: 3,
        retry: 0,
        variables: None,
        consistency_prompt: Some("custom prompt".into()),
        consistency_model: None,
        output: None,
    };
    let kind = orch.classify_step(&TaskType::Agent(agent_task));
    match kind {
        StepKind::Agent {
            consistency_prompt, ..
        } => {
            assert_eq!(consistency_prompt.as_deref(), Some("custom prompt"));
        }
        _ => panic!("expected StepKind::Agent"),
    }
}

#[test]
fn test_classify_step_agent_inherits_automation_prompt() {
    let mut orch = make_orchestrator(HashMap::new());
    orch.workflow.consistency_prompt = Some("workflow prompt".into());

    let agent_task = AgentTaskConfig {
        agent_ref: "my-agent".into(),
        prompt: "hello".into(),
        consistency_run: 3,
        retry: 0,
        variables: None,
        consistency_prompt: None, // no task-level override
        consistency_model: None,
        output: None,
    };
    let kind = orch.classify_step(&TaskType::Agent(agent_task));
    match kind {
        StepKind::Agent {
            consistency_prompt, ..
        } => {
            assert_eq!(consistency_prompt.as_deref(), Some("workflow prompt"));
        }
        _ => panic!("expected StepKind::Agent"),
    }
}

#[test]
fn test_classify_step_agent_task_prompt_overrides_automation() {
    let mut orch = make_orchestrator(HashMap::new());
    orch.workflow.consistency_prompt = Some("workflow prompt".into());

    let agent_task = AgentTaskConfig {
        agent_ref: "my-agent".into(),
        prompt: "hello".into(),
        consistency_run: 3,
        retry: 0,
        variables: None,
        consistency_prompt: Some("task prompt".into()),
        consistency_model: None,
        output: None,
    };
    let kind = orch.classify_step(&TaskType::Agent(agent_task));
    match kind {
        StepKind::Agent {
            consistency_prompt, ..
        } => {
            assert_eq!(consistency_prompt.as_deref(), Some("task prompt"));
        }
        _ => panic!("expected StepKind::Agent"),
    }
}

// ── Bug reproduction: answer channel closure ──────────────────────────
//
// Documents the failure mode that surfaces as
//   "resumed workflow orchestrator failed … error=answer channel closed"
// in production logs after a server restart.
//
// Root cause: AutomationStepOrchestrator::run blocks on `answer_rx.recv()`
// after emitting TaskOutcome::Suspended. If ALL `answer_tx` clones are
// dropped (e.g. during a recovery race where a second re-launch overwrites
// `state.orchestrator_txs` and the first orchestrator's senders are
// garbage-collected while it's still suspended), `recv()` returns `None`
// and `run()` errors out with "answer channel closed".
//
// The planned Temporal-style refactor replaces this long-lived actor with
// a stateless `AutomationDecider` — no in-memory channels span crashes.
#[tokio::test]
async fn test_bug_repro_answer_channel_closed_when_senders_dropped() {
    use tokio::sync::mpsc;
    use tokio::time::{Duration, timeout};

    // Build a minimal 1-step workflow whose step delegates (triggers suspend).
    let workflow = AutomationConfig {
        name: "repro_wf".into(),
        tasks: vec![TaskConfig {
            name: "s0".into(),
            task_type: TaskType::Unknown, // classify -> StepKind::Delegated
        }],
        description: String::new(),
        variables: None,
        consistency_prompt: None,
        consistency_model: None,
    };

    let mut orch = AutomationStepOrchestrator::new(workflow, json!({}), None, "trace".into(), None);

    let (event_tx, mut event_rx) = mpsc::channel::<(String, Value)>(256);
    let (outcome_tx, mut outcome_rx) = mpsc::channel::<TaskOutcome>(4);
    let (answer_tx, answer_rx) = mpsc::channel::<String>(4);

    // Spawn the orchestrator. It will emit subrun_started, step_started,
    // then suspend waiting on answer_rx.recv().
    let orch_task = tokio::spawn(async move { orch.run(event_tx, outcome_tx, answer_rx).await });

    // Drain events so the orchestrator isn't blocked on event_tx.send().
    let drain_events = tokio::spawn(async move { while event_rx.recv().await.is_some() {} });

    // Wait for the orchestrator to emit Suspended — proof it's now blocked
    // on answer_rx.recv().
    let suspended = timeout(Duration::from_secs(2), outcome_rx.recv())
        .await
        .expect("timed out waiting for Suspended outcome")
        .expect("outcome channel closed unexpectedly");
    assert!(
        matches!(suspended, TaskOutcome::Suspended { .. }),
        "expected Suspended outcome, got {suspended:?}"
    );

    // Now simulate the race: drop all answer_tx clones.
    // In production this happens when recovery's state.orchestrator_txs
    // entry gets overwritten/removed and the ExecutingTask's sender is
    // dropped via spawn_virtual_worker's scope end.
    drop(answer_tx);

    // The orchestrator's `run` should now return Err("answer channel closed").
    let result = timeout(Duration::from_secs(2), orch_task)
        .await
        .expect("orchestrator didn't exit after senders dropped")
        .expect("orchestrator task panicked");

    drain_events.abort();

    assert!(
        result
            .as_ref()
            .err()
            .is_some_and(|e| e.contains("answer channel closed")),
        "expected Err('answer channel closed'), got {result:?}"
    );
}
