//! A timeout or cancel whose terminal write fails is retried, not dropped.
//!
//! `check_suspend_timeouts` and `handle_cancelled` used to end their writes in
//! `.ok()`: one failed statement (a pool hiccup, a failover) and the run row
//! stayed `running` / `delegating` while the coordinator had already moved on —
//! a run nobody drives, reading as live. Each test below fails the exact
//! terminal UPDATE a few times with a trigger, then checks it landed anyway.
//!
//! The trigger counts its rejections with a sequence because `nextval` is not
//! rolled back with the failed statement; a counter row would be. Safe on the
//! shared database: this binary's tests run in nextest's `serial-db` group and
//! the trigger only matches one run id.
//!
//! Orchestrator hardening plan, Cleanup
//! (`internal-docs/2026-10-13-orchestrator-hardening-plan.md`).
//!
//! Run:
//!   cargo nextest run -p agentic-runtime --test integration -E 'test(terminal_write_test)'

use std::sync::Arc;
use std::time::Duration;

use agentic_core::delegation::{
    DelegationTarget, SuspendReason, TaskAssignment, TaskOutcome, TaskSpec,
};
use agentic_core::human_input::SuspendedRunData;
use agentic_core::transport::{CoordinatorTransport, WorkerTransport};
use agentic_runtime::coordinator::Coordinator;
use agentic_runtime::crud;
use agentic_runtime::state::{RunStatus, RuntimeState};
use agentic_runtime::transport::LocalTransport;
use agentic_runtime::worker::{ExecutingTask, TaskExecutor, Worker};
use async_trait::async_trait;
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::integration_tests::test_db;

/// Makes the next `times` UPDATEs that move `run_id` to `status` fail.
struct InjectedFailure {
    tag: String,
}

impl InjectedFailure {
    async fn arm(db: &DatabaseConnection, run_id: &str, status: &str, times: u32) -> Self {
        let tag = format!("tw_{}", uuid::Uuid::new_v4().simple());
        db.execute_unprepared(&format!(
            "CREATE SEQUENCE {tag}_seq; \
             CREATE FUNCTION {tag}_fn() RETURNS trigger AS $$ BEGIN \
               IF nextval('{tag}_seq') <= {times} THEN \
                 RAISE EXCEPTION 'injected terminal-write failure'; \
               END IF; \
               RETURN NEW; \
             END $$ LANGUAGE plpgsql; \
             CREATE TRIGGER {tag}_trg BEFORE UPDATE ON agentic_runs FOR EACH ROW \
               WHEN (NEW.id = '{run_id}' AND NEW.task_status = '{status}') \
               EXECUTE FUNCTION {tag}_fn();"
        ))
        .await
        .expect("arm the injected failure");
        Self { tag }
    }

    /// How many times the trigger fired (rejections plus the one let through).
    async fn fired(&self, db: &DatabaseConnection) -> i64 {
        let row = db
            .query_one_raw(sea_orm::Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                format!(
                    "SELECT CASE WHEN is_called THEN last_value ELSE 0 END AS n FROM {}_seq",
                    self.tag
                ),
            ))
            .await
            .unwrap()
            .unwrap();
        row.try_get::<i64>("", "n").unwrap()
    }

    async fn disarm(self, db: &DatabaseConnection) {
        let tag = self.tag;
        db.execute_unprepared(&format!(
            "DROP TRIGGER IF EXISTS {tag}_trg ON agentic_runs; \
             DROP FUNCTION IF EXISTS {tag}_fn(); \
             DROP SEQUENCE IF EXISTS {tag}_seq;"
        ))
        .await
        .expect("disarm the injected failure");
    }
}

/// Root agent `delegator` delegates to agent `hang`; agent `wait` (and `hang`)
/// runs until cancelled, `hang` finishing on its own after `hang_ms`. Every
/// `Resume` finishes.
struct Script {
    hang_ms: u64,
}

fn delegate_to_hang() -> TaskOutcome {
    TaskOutcome::Suspended {
        reason: SuspendReason::Delegation {
            target: DelegationTarget::Agent {
                agent_id: "hang".into(),
            },
            request: "hang".into(),
            context: json!({}),
            policy: None,
        },
        resume_data: SuspendedRunData {
            from_state: "executing".into(),
            original_input: "root".into(),
            trace_id: "trace-1".into(),
            stage_data: json!({}),
            question: "delegate".into(),
            suggestions: vec![],
        },
        trace_id: "trace-1".into(),
    }
}

fn done() -> TaskOutcome {
    TaskOutcome::Done {
        answer: "done".into(),
        metadata: None,
    }
}

#[async_trait]
impl TaskExecutor for Script {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let (_event_tx, events) = mpsc::channel(1);
        let (outcome_tx, outcomes) = mpsc::channel(1);
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let hang = Duration::from_millis(self.hang_ms);
        tokio::spawn(async move {
            let outcome = match assignment.spec {
                TaskSpec::Agent { agent_id, .. } if agent_id == "delegator" => delegate_to_hang(),
                TaskSpec::Agent { .. } => tokio::select! {
                    _ = token.cancelled() => TaskOutcome::Cancelled,
                    _ = tokio::time::sleep(hang) => done(),
                },
                _ => done(),
            };
            let _ = outcome_tx.send(outcome).await;
        });
        Ok(ExecutingTask {
            events,
            outcomes,
            cancel,
            answers: None,
        })
    }
}

/// A started root (`agent_id`) and its coordinator, running.
struct Driven {
    run_id: String,
    state: Arc<RuntimeState>,
    transport: Arc<LocalTransport>,
    coordinator: tokio::task::JoinHandle<()>,
}

async fn start(
    db: &DatabaseConnection,
    agent_id: &str,
    hang_ms: u64,
    suspend_timeout: Option<Duration>,
) -> (Driven, mpsc::Sender<String>) {
    let run_id = format!("tw-{}", uuid::Uuid::new_v4());
    crud::insert_run(
        db,
        &run_id,
        "root",
        None,
        "analytics",
        None,
        uuid::Uuid::nil(),
    )
    .await
    .unwrap();
    let state = Arc::new(RuntimeState::new());
    let transport = LocalTransport::with_defaults();
    let worker = Worker::new(
        transport.clone() as Arc<dyn WorkerTransport>,
        Arc::new(Script { hang_ms }),
    );
    let (answer_tx, answer_rx) = mpsc::channel::<String>(1);
    let mut coordinator = Coordinator::new(
        db.clone(),
        state.clone(),
        transport.clone() as Arc<dyn CoordinatorTransport>,
    );
    if let Some(timeout) = suspend_timeout {
        coordinator = coordinator.with_suspend_timeout(timeout);
    }
    coordinator.register_answer_channel(run_id.clone(), answer_rx);
    let spec = TaskSpec::Agent {
        agent_id: agent_id.into(),
        question: "root".into(),
        extra: None,
    };
    coordinator.submit_root(run_id.clone(), spec).await.unwrap();
    tokio::spawn(async move { worker.run().await });
    let coordinator = tokio::spawn(async move { coordinator.run().await });
    let driven = Driven {
        run_id,
        state,
        transport,
        coordinator,
    };
    (driven, answer_tx)
}

async fn finish(driven: Driven) {
    tokio::time::timeout(Duration::from_secs(30), driven.coordinator)
        .await
        .expect("the coordinator never finished")
        .expect("coordinator panicked");
}

async fn status_of(db: &DatabaseConnection, run_id: &str) -> Option<String> {
    crud::get_run(db, run_id)
        .await
        .unwrap()
        .unwrap()
        .task_status
}

#[tokio::test]
async fn a_root_cancel_whose_write_fails_twice_still_lands() {
    let Some(db) = test_db().await else {
        return;
    };
    let (driven, _answers) = start(&db, "wait", 60_000, None).await;
    let run_id = driven.run_id.clone();
    let failure = InjectedFailure::arm(&db, &run_id, "cancelled", 2).await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    driven.transport.cancel(&run_id).await.unwrap();
    finish(driven).await;

    let fired = failure.fired(&db).await;
    failure.disarm(&db).await;
    assert_eq!(status_of(&db, &run_id).await.as_deref(), Some("cancelled"));
    assert_eq!(fired, 3, "two rejected writes, then the one that landed");
}

#[tokio::test]
async fn a_root_timeout_whose_write_fails_twice_still_lands() {
    let Some(db) = test_db().await else {
        return;
    };
    let (driven, _answers) = start(&db, "delegator", 3_000, Some(Duration::from_secs(1))).await;
    let run_id = driven.run_id.clone();
    let failure = InjectedFailure::arm(&db, &run_id, "timed_out", 2).await;

    finish(driven).await;

    let fired = failure.fired(&db).await;
    failure.disarm(&db).await;
    assert_eq!(status_of(&db, &run_id).await.as_deref(), Some("timed_out"));
    assert_eq!(fired, 3, "two rejected writes, then the one that landed");
}

/// The child half of `handle_cancelled` writes the child's status and its
/// outcome row in one transaction; a retried transaction must land both.
#[tokio::test]
async fn a_child_cancel_whose_write_fails_twice_still_records_its_outcome() {
    let Some(db) = test_db().await else {
        return;
    };
    let (driven, _answers) = start(&db, "delegator", 60_000, None).await;
    let child = format!("{}.1", driven.run_id);
    let failure = InjectedFailure::arm(&db, &child, "cancelled", 2).await;

    tokio::time::sleep(Duration::from_millis(500)).await;
    driven.transport.cancel(&child).await.unwrap();
    let run_id = driven.run_id.clone();
    finish(driven).await;

    let fired = failure.fired(&db).await;
    failure.disarm(&db).await;
    assert_eq!(status_of(&db, &child).await.as_deref(), Some("cancelled"));
    let outcomes = crud::get_outcomes_for_parent(&db, &run_id).await.unwrap();
    assert_eq!(
        outcomes
            .iter()
            .map(|o| o.status.as_str())
            .collect::<Vec<_>>(),
        vec!["cancelled"],
        "the child's outcome row commits with its status"
    );
    assert_eq!(fired, 3, "two rejected writes, then the one that landed");
}

/// A database that never takes the write: the coordinator still ends and says
/// what it knows in memory, rather than hanging on a write it cannot make.
#[tokio::test]
async fn a_root_cancel_whose_write_never_lands_still_ends_the_coordinator() {
    let Some(db) = test_db().await else {
        return;
    };
    let (driven, _answers) = start(&db, "wait", 60_000, None).await;
    let run_id = driven.run_id.clone();
    let state = driven.state.clone();
    let failure = InjectedFailure::arm(&db, &run_id, "cancelled", 1_000).await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    driven.transport.cancel(&run_id).await.unwrap();
    finish(driven).await;

    let fired = failure.fired(&db).await;
    failure.disarm(&db).await;
    assert!(
        matches!(
            state.statuses.get(&run_id).map(|s| s.clone()),
            Some(RunStatus::Cancelled)
        ),
        "the in-memory status still reports the cancel"
    );
    assert_ne!(status_of(&db, &run_id).await.as_deref(), Some("cancelled"));
    assert!(
        fired > 1,
        "the write was retried, not tried once: fired {fired}"
    );
}
