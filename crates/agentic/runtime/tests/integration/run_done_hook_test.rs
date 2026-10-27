//! A root run the coordinator writes `done` tells the host's run-done observer,
//! once, after the status is committed. A request to hear about a run that
//! already ended tells the host straight away — through the request's own
//! port, never the run-end observers — and one for a run still going waits.
//! A root run that stops to ask the person something tells the suspend
//! observer once the suspension is written.
//!
//! Run:
//!   cargo nextest run -p agentic-runtime --test integration -E 'test(run_done_hook_test)'

use std::sync::{Arc, Mutex};
use std::time::Duration;

use agentic_core::delegation::{SuspendReason, TaskAssignment, TaskOutcome, TaskSpec};
use agentic_core::events::HumanInputQuestion;
use agentic_core::human_input::SuspendedRunData;
use agentic_core::transport::{CoordinatorTransport, WorkerTransport};
use agentic_runtime::coordinator::Coordinator;
use agentic_runtime::crud;
use agentic_runtime::lifecycle::done_hook::{RunDoneObserver, set_run_done_observer};
use agentic_runtime::lifecycle::failure_hook::{RunFailureObserver, set_run_failure_observer};
use agentic_runtime::lifecycle::notice_request::{
    NoticeRequestObserver, notice_requested, request_notice, set_notice_request_observer,
};
use agentic_runtime::lifecycle::suspend_hook::{RunSuspendedObserver, set_run_suspended_observer};
use agentic_runtime::orchestrator::transport::DurableTransport;
use agentic_runtime::state::RuntimeState;
use agentic_runtime::worker::{ExecutingTask, TaskExecutor, Worker};
use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::eviction_safety_test::test_db;

/// Generous: the machine running these is heavily loaded.
const SETTLE: Duration = Duration::from_secs(30);

/// What one port was told, with the run's status as it read it then.
#[derive(Default)]
struct Seen(Mutex<Vec<(String, Option<String>)>>);

impl Seen {
    async fn record(&self, db: &DatabaseConnection, run_id: &str) {
        let status = crud::get_run(db, run_id)
            .await
            .unwrap()
            .and_then(|r| r.task_status);
        self.0.lock().unwrap().push((run_id.to_string(), status));
    }

    fn all(&self) -> Vec<(String, Option<String>)> {
        self.0.lock().unwrap().clone()
    }
}

/// Installed on all four ports, keeping each one's calls apart.
struct Recorder {
    db: DatabaseConnection,
    done: Seen,
    failed: Seen,
    requested: Seen,
    suspended: Seen,
}

impl Recorder {
    fn install(db: &DatabaseConnection) -> Arc<Self> {
        let recorder = Arc::new(Self {
            db: db.clone(),
            done: Seen::default(),
            failed: Seen::default(),
            requested: Seen::default(),
            suspended: Seen::default(),
        });
        set_run_done_observer(recorder.clone());
        set_run_failure_observer(recorder.clone());
        set_notice_request_observer(recorder.clone());
        set_run_suspended_observer(recorder.clone());
        recorder
    }

    fn seen(&self) -> Vec<(String, Option<String>)> {
        self.done.all()
    }
}

#[async_trait]
impl RunDoneObserver for Recorder {
    async fn root_run_done(&self, run_id: &str) {
        self.done.record(&self.db, run_id).await;
    }
}

#[async_trait]
impl RunFailureObserver for Recorder {
    async fn root_run_failed(&self, run_id: &str) {
        self.failed.record(&self.db, run_id).await;
    }
}

#[async_trait]
impl RunSuspendedObserver for Recorder {
    async fn root_run_suspended(&self, run_id: &str) {
        self.suspended.record(&self.db, run_id).await;
    }
}

#[async_trait]
impl NoticeRequestObserver for Recorder {
    async fn requested_after_end(&self, run_id: &str) {
        self.requested.record(&self.db, run_id).await;
    }
}

/// Answers at once.
struct Answers;

#[async_trait]
impl TaskExecutor for Answers {
    async fn execute(&self, _a: TaskAssignment) -> Result<ExecutingTask, String> {
        let (_event_tx, events) = mpsc::channel(1);
        let (outcome_tx, outcomes) = mpsc::channel(1);
        outcome_tx
            .send(TaskOutcome::Done {
                answer: "42".into(),
                metadata: None,
            })
            .await
            .unwrap();
        Ok(ExecutingTask {
            events,
            outcomes,
            cancel: CancellationToken::new(),
            answers: None,
        })
    }
}

#[tokio::test]
async fn a_root_run_done_tells_the_observer_after_its_status_is_written() {
    let db = test_db().await.expect("test db");
    let recorder = Recorder::install(&db);
    let run_id = format!("done-hook-{}", uuid::Uuid::new_v4());
    crud::insert_run(
        &db,
        &run_id,
        "q",
        None,
        "analytics",
        None,
        uuid::Uuid::nil(),
    )
    .await
    .unwrap();

    let transport = DurableTransport::new_scoped(db.clone(), run_id.clone());
    let mut coordinator = Coordinator::new(
        db.clone(),
        Arc::new(RuntimeState::new()),
        transport.clone() as Arc<dyn CoordinatorTransport>,
    );
    coordinator
        .submit_root(
            run_id.clone(),
            TaskSpec::Agent {
                agent_id: "test-agent".into(),
                question: "q".into(),
                extra: None,
            },
        )
        .await
        .unwrap();
    let worker = Worker::new(
        transport.clone() as Arc<dyn WorkerTransport>,
        Arc::new(Answers),
    );
    let worker_handle = tokio::spawn(async move { worker.run().await });
    let finished = tokio::time::timeout(SETTLE, async move { coordinator.run().await })
        .await
        .is_ok();
    transport.retire_worker_loop();
    let _ = tokio::time::timeout(Duration::from_secs(5), worker_handle).await;

    assert!(finished, "the coordinator did not finish the run");
    assert_eq!(
        recorder.seen(),
        vec![(run_id, Some("done".to_string()))],
        "told once, and the run already reads done"
    );
}

#[tokio::test]
async fn a_request_for_a_run_that_already_ended_tells_the_request_port_at_once() {
    let db = test_db().await.expect("test db");
    let recorder = Recorder::install(&db);
    let going = format!("notice-going-{}", uuid::Uuid::new_v4());
    let done = format!("notice-done-{}", uuid::Uuid::new_v4());
    let failed = format!("notice-failed-{}", uuid::Uuid::new_v4());
    for id in [&going, &done, &failed] {
        crud::insert_run(&db, id, "q", None, "analytics", None, uuid::Uuid::nil())
            .await
            .unwrap();
    }
    crud::update_run_done(&db, &done, "42", None).await.unwrap();
    crud::update_run_failed(&db, &failed, "boom").await.unwrap();

    assert!(!notice_requested(&db, &going).await.unwrap());
    assert!(request_notice(&db, &going).await.unwrap());
    assert!(notice_requested(&db, &going).await.unwrap());
    assert!(
        recorder.requested.all().is_empty(),
        "still going: told when it ends"
    );

    assert!(request_notice(&db, &done).await.unwrap());
    assert!(request_notice(&db, &failed).await.unwrap());
    assert_eq!(
        recorder.requested.all(),
        vec![
            (done.clone(), Some("done".into())),
            (failed.clone(), Some("failed".into())),
        ]
    );
    // The run-end observers announce a run's end to whoever it concerns, by
    // email and Slack too; a request may not set them off again.
    assert!(recorder.done.all().is_empty(), "the done observer");
    assert!(recorder.failed.all().is_empty(), "the failure observer");

    assert!(
        !request_notice(&db, "no-such-run").await.unwrap(),
        "nothing to record"
    );
}

/// Stops at once to ask the person a question.
struct AsksFirst;

#[async_trait]
impl TaskExecutor for AsksFirst {
    async fn execute(&self, _a: TaskAssignment) -> Result<ExecutingTask, String> {
        let (_event_tx, events) = mpsc::channel(1);
        let (outcome_tx, outcomes) = mpsc::channel(1);
        let question = "Which region do you mean?".to_string();
        outcome_tx
            .send(TaskOutcome::Suspended {
                reason: SuspendReason::HumanInput {
                    questions: vec![HumanInputQuestion {
                        prompt: question.clone(),
                        suggestions: vec!["West".into()],
                    }],
                },
                resume_data: SuspendedRunData {
                    from_state: "clarifying".into(),
                    original_input: "q".into(),
                    trace_id: String::new(),
                    stage_data: serde_json::Value::Null,
                    question,
                    suggestions: vec!["West".into()],
                },
                trace_id: String::new(),
            })
            .await
            .unwrap();
        Ok(ExecutingTask {
            events,
            outcomes,
            cancel: CancellationToken::new(),
            answers: None,
        })
    }
}

#[tokio::test]
async fn a_root_run_that_stops_for_input_tells_the_suspend_observer() {
    let db = test_db().await.expect("test db");
    let recorder = Recorder::install(&db);
    let run_id = format!("suspend-hook-{}", uuid::Uuid::new_v4());
    crud::insert_run(
        &db,
        &run_id,
        "q",
        None,
        "analytics",
        None,
        uuid::Uuid::nil(),
    )
    .await
    .unwrap();

    let transport = DurableTransport::new_scoped(db.clone(), run_id.clone());
    let mut coordinator = Coordinator::new(
        db.clone(),
        Arc::new(RuntimeState::new()),
        transport.clone() as Arc<dyn CoordinatorTransport>,
    );
    coordinator
        .submit_root(
            run_id.clone(),
            TaskSpec::Agent {
                agent_id: "test-agent".into(),
                question: "q".into(),
                extra: None,
            },
        )
        .await
        .unwrap();
    let worker = Worker::new(
        transport.clone() as Arc<dyn WorkerTransport>,
        Arc::new(AsksFirst),
    );
    let worker_handle = tokio::spawn(async move { worker.run().await });
    // A suspended run waits on its answer, so the coordinator does not finish:
    // watch the observer instead.
    let coordinator_handle = tokio::spawn(async move { coordinator.run().await });
    let deadline = tokio::time::Instant::now() + SETTLE;
    while recorder.suspended.all().is_empty() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    coordinator_handle.abort();
    transport.retire_worker_loop();
    let _ = tokio::time::timeout(Duration::from_secs(5), worker_handle).await;

    assert_eq!(
        recorder.suspended.all(),
        vec![(run_id, Some("awaiting_input".to_string()))],
        "told once, after the suspension was written"
    );
    assert!(recorder.done.all().is_empty() && recorder.failed.all().is_empty());
}
