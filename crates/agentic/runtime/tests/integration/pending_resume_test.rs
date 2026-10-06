//! What `Coordinator::from_db` reports as "every child in", and what a caller
//! may do with it.
//!
//! A `PendingResume` is a parent the rebuilt coordinator will resume with its
//! children's aggregated answer. Two things have to be true of it: the children
//! it counts are the ones the parent is waiting on *now*, and the caller can
//! tell whether a resume is possible at all. And when the resume was already
//! assigned before the crash, the caller must be able to say so
//! (`already_assigned`), leaving the coordinator with a running task rather
//! than one still waiting on children.
//!
//! Run:
//!   cargo nextest run -p agentic-runtime --test integration -E 'test(pending_resume_test)'

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use agentic_core::delegation::{
    DelegationTarget, SuspendReason, TaskAssignment, TaskOutcome, TaskSpec,
};
use agentic_core::human_input::SuspendedRunData;
use agentic_core::transport::{CoordinatorTransport, WorkerTransport};
use agentic_runtime::coordinator::{Coordinator, PendingResume};
use agentic_runtime::crud;
use agentic_runtime::state::RuntimeState;
use agentic_runtime::transport::LocalTransport;
use agentic_runtime::worker::{ExecutingTask, TaskExecutor, Worker};
use async_trait::async_trait;
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::integration_tests::test_db;

fn checkpoint() -> SuspendedRunData {
    SuspendedRunData {
        from_state: "executing".into(),
        original_input: "how did the stores do?".into(),
        trace_id: "trace-1".into(),
        stage_data: json!({}),
        question: "delegate this".into(),
        suggestions: vec![],
    }
}

/// A root parked at a delegation to `children`, as the coordinator writes it.
async fn parent_waiting_on(db: &DatabaseConnection, children: &[&str]) -> String {
    let parent = format!("pending-{}", uuid::Uuid::new_v4());
    crud::insert_run(db, &parent, "Q", None, "analytics", None, uuid::Uuid::nil())
        .await
        .unwrap();
    let ids: Vec<String> = children.iter().map(|c| format!("{parent}.{c}")).collect();
    crud::suspend_with_data_txn(
        db,
        &parent,
        "delegating",
        Some(json!({
            "child_task_ids": ids,
            "completed": {},
            "failure_policy": "fail_fast",
        })),
        "delegate this",
        &[],
        &checkpoint(),
    )
    .await
    .unwrap();
    parent
}

/// A child of `parent` that has finished and reported `answer`.
async fn finished_child(db: &DatabaseConnection, parent: &str, n: &str, answer: &str) {
    let child = format!("{parent}.{n}");
    crud::insert_child_run(db, &child, parent, "child Q", "analytics", 0, None)
        .await
        .unwrap();
    crud::complete_child_done_txn(db, &child, &child, parent, answer)
        .await
        .unwrap();
}

async fn rebuild(db: &DatabaseConnection, root: &str) -> (Coordinator, Vec<PendingResume>) {
    let transport = LocalTransport::with_defaults();
    Coordinator::from_db(
        db.clone(),
        Arc::new(RuntimeState::new()),
        transport as Arc<dyn CoordinatorTransport>,
        root,
    )
    .await
    .unwrap()
}

/// `agentic_task_outcomes` keeps every outcome a parent was ever sent. A
/// parent on its second delegation still has the first one's row, and one old
/// outcome against one expected child is not "every child in".
#[tokio::test]
async fn an_earlier_delegations_outcome_does_not_complete_the_current_one() {
    let Some(db) = test_db().await else {
        return;
    };
    let parent = parent_waiting_on(&db, &["2"]).await;
    finished_child(&db, &parent, "1", "first delegation's answer").await;
    crud::insert_child_run(
        &db,
        &format!("{parent}.2"),
        &parent,
        "child Q",
        "analytics",
        0,
        None,
    )
    .await
    .unwrap();

    let (_coordinator, pending) = rebuild(&db, &parent).await;
    assert!(
        pending.is_empty(),
        "the child the parent is waiting on has not reported: {pending:?}"
    );

    // Once it does, the parent is pending, with that child's answer alone.
    let second = format!("{parent}.2");
    crud::complete_child_done_txn(&db, &second, &second, &parent, "second delegation's answer")
        .await
        .unwrap();
    let (_coordinator, pending) = rebuild(&db, &parent).await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].parent_task_id, parent);
    assert_eq!(pending[0].answer, "second delegation's answer");
}

/// A pending parent with no readable checkpoint cannot be resumed, and the
/// caller deciding who continues it has to be told.
#[tokio::test]
async fn a_pending_resume_says_whether_a_checkpoint_backs_it() {
    let Some(db) = test_db().await else {
        return;
    };
    let parent = parent_waiting_on(&db, &["1"]).await;
    finished_child(&db, &parent, "1", "child answer").await;

    let (_coordinator, pending) = rebuild(&db, &parent).await;
    assert_eq!(pending.len(), 1);
    assert!(pending[0].has_checkpoint);

    db.execute_raw(Statement::from_sql_and_values(
        db.get_database_backend(),
        "UPDATE agentic_run_suspensions SET resume_data = '{}'::jsonb WHERE run_id = $1",
        [parent.clone().into()],
    ))
    .await
    .unwrap();
    let (_coordinator, pending) = rebuild(&db, &parent).await;
    assert_eq!(pending.len(), 1, "still every child in");
    assert!(!pending[0].has_checkpoint);
}

/// The parent's first `Resume` delegates again; everything after it finishes.
struct DelegatesOnceMore {
    resumes: AtomicUsize,
}

#[async_trait]
impl TaskExecutor for DelegatesOnceMore {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let (_event_tx, events) = mpsc::channel(1);
        let (outcome_tx, outcomes) = mpsc::channel(1);
        let first_resume = matches!(assignment.spec, TaskSpec::Resume { .. })
            && self.resumes.fetch_add(1, Ordering::SeqCst) == 0;
        let outcome = if first_resume {
            TaskOutcome::Suspended {
                reason: SuspendReason::Delegation {
                    target: DelegationTarget::Automation {
                        workflow_ref: "weekly.automation.yml".into(),
                    },
                    request: "one more report".into(),
                    context: json!({}),
                    policy: None,
                },
                resume_data: checkpoint(),
                trace_id: "trace-1".into(),
            }
        } else {
            TaskOutcome::Done {
                answer: "done".into(),
                metadata: None,
            }
        };
        let _ = outcome_tx.send(outcome).await;
        Ok(ExecutingTask {
            events,
            outcomes,
            cancel: CancellationToken::new(),
            answers: None,
        })
    }
}

/// The resume was assigned before the crash and survived on the queue. Told
/// so, the coordinator treats the parent as running: when the resumed task
/// delegates again, that is a new delegation and gets its child. Left as
/// rebuilt — still waiting on the children that already reported — the
/// coordinator would drop it as a duplicate and the run would never move.
#[tokio::test]
async fn a_parent_whose_resume_is_already_assigned_is_running() {
    let Some(db) = test_db().await else {
        return;
    };
    let parent = parent_waiting_on(&db, &["1"]).await;
    finished_child(&db, &parent, "1", "child answer").await;

    let state = Arc::new(RuntimeState::new());
    let transport = LocalTransport::with_defaults();
    let (mut coordinator, mut pending) = Coordinator::from_db(
        db.clone(),
        state,
        transport.clone() as Arc<dyn CoordinatorTransport>,
        &parent,
    )
    .await
    .unwrap();
    assert_eq!(pending.len(), 1, "rebuilt as waiting, every child in");
    assert!(!pending[0].already_assigned, "`from_db` cannot know");

    // The caller's part: the resume is on the queue already — here, put there
    // by hand — so the pending resume is marked rather than assigned again.
    pending[0].already_assigned = true;
    coordinator.process_pending_resumes(pending).await;
    transport
        .assign(TaskAssignment {
            task_id: parent.clone(),
            parent_task_id: None,
            run_id: parent.clone(),
            spec: TaskSpec::Resume {
                run_id: parent.clone(),
                resume_data: checkpoint(),
                answer: "child answer".into(),
            },
            policy: None,
        })
        .await
        .unwrap();

    let worker = Worker::new(
        transport.clone() as Arc<dyn WorkerTransport>,
        Arc::new(DelegatesOnceMore {
            resumes: AtomicUsize::new(0),
        }),
    );
    tokio::spawn(async move { worker.run().await });
    let driven = tokio::spawn(async move { coordinator.run().await });
    tokio::time::timeout(Duration::from_secs(20), driven)
        .await
        .expect(
            "the run never finished: the resumed parent's new delegation was dropped as a \
             duplicate of the one whose children had already reported",
        )
        .expect("coordinator panicked");

    let tree = crud::load_task_tree(&db, &parent).await.unwrap();
    assert!(
        tree.iter().any(|r| r.id == format!("{parent}.2")),
        "the new delegation got its child: {:?}",
        tree.iter().map(|r| &r.id).collect::<Vec<_>>()
    );
    let root = crud::get_run(&db, &parent).await.unwrap().unwrap();
    assert_eq!(root.task_status.as_deref(), Some("done"));
}
