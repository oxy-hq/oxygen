//! A child that itself delegates keeps the `original_spec` its parent stored.
//!
//! `insert_child_run` writes `task_metadata = {policy, original_spec}` on a
//! delegated child. When that child delegates in turn, the coordinator persists
//! its fan-out state (`child_task_ids`, `completed`, `failure_policy`) onto the
//! same column, at the suspension and again as each grandchild result is
//! folded into `completed`. Both writes used to *replace* the column, so the
//! child's `original_spec` was gone by the time its grandchild finished — and resuming
//! it is exactly the read that needs it: a delegated run has `metadata = None`,
//! so the pipeline executor recovers its `agent_id` from
//! `task_metadata.original_spec.agent_id`, found `""`, and failed with
//! "resume: agent_id is empty".
//!
//! Orchestrator hardening plan, Cleanup
//! (`internal-docs/2026-10-13-orchestrator-hardening-plan.md`).
//!
//! Run:
//!   cargo nextest run -p agentic-runtime --test integration -E 'test(delegating_child_metadata_test)'

use std::sync::{Arc, Mutex};
use std::time::Duration;

use agentic_core::delegation::{
    DelegationTarget, SuspendReason, TaskAssignment, TaskOutcome, TaskSpec,
};
use agentic_core::human_input::SuspendedRunData;
use agentic_core::transport::{CoordinatorTransport, WorkerTransport};
use agentic_runtime::coordinator::Coordinator;
use agentic_runtime::crud;
use agentic_runtime::state::RuntimeState;
use agentic_runtime::transport::LocalTransport;
use agentic_runtime::worker::{ExecutingTask, TaskExecutor, Worker};
use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::integration_tests::test_db;

fn delegate(agent_id: &str, request: &str) -> TaskOutcome {
    TaskOutcome::Suspended {
        reason: SuspendReason::Delegation {
            target: DelegationTarget::Agent {
                agent_id: agent_id.into(),
            },
            request: request.into(),
            context: json!({}),
            policy: None,
        },
        resume_data: SuspendedRunData {
            from_state: "executing".into(),
            original_input: request.into(),
            trace_id: "trace-1".into(),
            stage_data: json!({}),
            question: "delegate".into(),
            suggestions: vec![],
        },
        trace_id: "trace-1".into(),
    }
}

/// `root` delegates to agent `middle-agent`, which delegates to `leaf-agent`.
/// On every `Resume` it reads the resumed run's row the way the pipeline
/// executor does, and records the `agent_id` it would resolve.
struct ThreeLevels {
    db: DatabaseConnection,
    resolved: Arc<Mutex<Vec<(String, Option<String>)>>>,
}

impl ThreeLevels {
    async fn outcome_for(&self, spec: TaskSpec) -> TaskOutcome {
        match spec {
            TaskSpec::Agent { agent_id, .. } if agent_id == "root-agent" => {
                delegate("middle-agent", "middle")
            }
            TaskSpec::Agent { agent_id, .. } if agent_id == "middle-agent" => {
                delegate("leaf-agent", "leaf")
            }
            TaskSpec::Agent { .. } => done("leaf answer"),
            TaskSpec::Resume { run_id, .. } => {
                let row = crud::get_run(&self.db, &run_id).await.unwrap().unwrap();
                let agent_id = row
                    .task_metadata
                    .as_ref()
                    .and_then(|m| m.get("original_spec"))
                    .and_then(|s| s.get("agent_id"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                self.resolved.lock().unwrap().push((run_id, agent_id));
                done("resumed")
            }
            other => panic!("unexpected spec: {other:?}"),
        }
    }
}

fn done(answer: &str) -> TaskOutcome {
    TaskOutcome::Done {
        answer: answer.into(),
        metadata: None,
    }
}

#[async_trait]
impl TaskExecutor for ThreeLevels {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let (_event_tx, events) = mpsc::channel(1);
        let (outcome_tx, outcomes) = mpsc::channel(1);
        let outcome = self.outcome_for(assignment.spec).await;
        tokio::spawn(async move {
            let _ = outcome_tx.send(outcome).await;
        });
        Ok(ExecutingTask {
            events,
            outcomes,
            cancel: CancellationToken::new(),
            answers: None,
        })
    }
}

#[tokio::test]
async fn a_delegating_child_keeps_its_original_spec() {
    let Some(db) = test_db().await else {
        return;
    };
    let run_id = format!("meta-{}", uuid::Uuid::new_v4());
    crud::insert_run(
        &db,
        &run_id,
        "root",
        None,
        "workflow",
        None,
        uuid::Uuid::nil(),
    )
    .await
    .unwrap();
    let resolved = Arc::new(Mutex::new(vec![]));
    let transport = LocalTransport::with_defaults();
    let executor = ThreeLevels {
        db: db.clone(),
        resolved: resolved.clone(),
    };
    let worker = Worker::new(
        transport.clone() as Arc<dyn WorkerTransport>,
        Arc::new(executor),
    );
    let mut coordinator = Coordinator::new(
        db.clone(),
        Arc::new(RuntimeState::new()),
        transport.clone() as Arc<dyn CoordinatorTransport>,
    );
    let root_spec = TaskSpec::Agent {
        agent_id: "root-agent".into(),
        question: "root".into(),
        extra: None,
    };
    coordinator
        .submit_root(run_id.clone(), root_spec)
        .await
        .unwrap();
    tokio::spawn(async move { worker.run().await });
    let driven = tokio::spawn(async move { coordinator.run().await });
    tokio::time::timeout(Duration::from_secs(30), driven)
        .await
        .expect("the coordinator never finished")
        .expect("coordinator panicked");

    let middle = format!("{run_id}.1");
    let seen = resolved.lock().unwrap().clone();
    let middle_resolved = seen.iter().find(|(id, _)| *id == middle);
    assert_eq!(
        middle_resolved.map(|(_, agent)| agent.as_deref()),
        Some(Some("middle-agent")),
        "resuming the delegating child must still find its agent_id; saw {seen:?}"
    );

    let row = crud::get_run(&db, &middle).await.unwrap().unwrap();
    let meta = row
        .task_metadata
        .expect("the child keeps its task_metadata");
    assert_eq!(
        meta["original_spec"]["agent_id"], "middle-agent",
        "original_spec survives the child's own delegation: {meta}"
    );
    assert_eq!(
        meta["child_task_ids"],
        json!([format!("{middle}.2")]),
        "and the delegation state is still recorded beside it: {meta}"
    );
    assert_eq!(row.task_status.as_deref(), Some("done"));
    assert_eq!(
        crud::get_run(&db, &run_id)
            .await
            .unwrap()
            .unwrap()
            .task_status
            .as_deref(),
        Some("done")
    );
}
