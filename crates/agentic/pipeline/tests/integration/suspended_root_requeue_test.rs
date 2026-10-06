//! A root that has reached a suspension is continued **from it** — resumed, or
//! left parked — and never also started again by its own queue entry.
//!
//! A suspended task keeps its claim so the same `task_id` can resume in place.
//! When the driver holding that claim dies, the reaper (or a graceful release)
//! puts the row back to `queued` carrying the spec it was claimed with: the one
//! the run was *started* with. Recovery's tree walk then continued the root
//! from its suspension and left that row for its worker, which claimed it and
//! ran the start spec from the top — a second copy of the run beside the first.
//!
//! The probe below is a `TaskSpec::Custom` executor that records every start it
//! is asked to make, so "the run was started again" is observed, not inferred.
//!
//! Run:
//!   cargo nextest run -p agentic-pipeline --test integration -E 'test(suspended_root_requeue_test)'

use std::sync::{Arc, Mutex};
use std::time::Duration;

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_core::human_input::SuspendedRunData;
use agentic_pipeline::platform::{IdentityResolver, PlatformContext};
use agentic_pipeline::recovery::DrivePolicy;
use agentic_runtime::crud;
use agentic_runtime::state::RuntimeState;
use agentic_runtime::worker::{CustomTaskRegistry, ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::automation_recovery_test::{FakePlatform, test_db};

pub(crate) const PROBE_KIND: &str = "suspended_root_probe";

/// Records every run it is asked to start, then reports it done.
#[derive(Default)]
pub(crate) struct Probe {
    started: Mutex<Vec<String>>,
}

impl Probe {
    pub(crate) fn started(&self, run_id: &str) -> bool {
        self.started.lock().unwrap().iter().any(|r| r == run_id)
    }
}

#[async_trait]
impl TaskExecutor for Probe {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        self.started.lock().unwrap().push(assignment.run_id.clone());
        let (_event_tx, events) = mpsc::channel(1);
        let (outcome_tx, outcomes) = mpsc::channel(1);
        let _ = outcome_tx
            .send(TaskOutcome::Done {
                answer: "started from the top".into(),
                metadata: None,
            })
            .await;
        Ok(ExecutingTask {
            events,
            outcomes,
            cancel: CancellationToken::new(),
            answers: None,
        })
    }
}

fn registry(probe: &Arc<Probe>) -> Arc<CustomTaskRegistry> {
    let mut registry = CustomTaskRegistry::new();
    registry.register(PROBE_KIND, probe.clone());
    Arc::new(registry)
}

fn start_spec() -> TaskSpec {
    TaskSpec::Custom {
        kind: PROBE_KIND.into(),
        payload: json!({}),
    }
}

pub(crate) fn checkpoint(question: &str) -> SuspendedRunData {
    SuspendedRunData {
        from_state: "clarifying".into(),
        original_input: "how did the stores do?".into(),
        trace_id: "trace-1".into(),
        stage_data: json!({ "asked": question }),
        question: question.into(),
        suggestions: vec![],
    }
}

/// A Global run as a seed leaves it: its row `running`, its start spec `queued`.
pub(crate) async fn seed(db: &DatabaseConnection, ws: Uuid) -> String {
    let run_id = Uuid::new_v4().to_string();
    crud::insert_run(db, &run_id, "q", None, PROBE_KIND, None, ws)
        .await
        .expect("insert run");
    requeue_with(db, &run_id, &start_spec()).await;
    run_id
}

/// The run's entry as a reaped or released claim leaves it: `queued` again,
/// carrying `spec`.
pub(crate) async fn requeue_with(db: &DatabaseConnection, run_id: &str, spec: &TaskSpec) {
    crud::enqueue_task(
        db,
        run_id,
        run_id,
        None,
        spec,
        None,
        crud::TaskScope::Global,
    )
    .await
    .expect("queue the root");
}

/// Park `run_id` on a clarifying question, as the coordinator does.
async fn park_on_a_question(db: &DatabaseConnection, run_id: &str) {
    crud::suspend_with_data_txn(
        db,
        run_id,
        "awaiting_input",
        None,
        "which store?",
        &[],
        &checkpoint("which store?"),
    )
    .await
    .expect("park the run");
}

/// Park `run_id` on one child that is itself waiting on a person, so nothing
/// in the tree moves by itself.
async fn park_on_a_child(db: &DatabaseConnection, run_id: &str) {
    let child = format!("{run_id}.1");
    crud::suspend_with_data_txn(
        db,
        run_id,
        "delegating",
        Some(json!({
            "child_task_ids": [&child],
            "completed": {},
            "failure_policy": "fail_fast",
        })),
        "Delegation to weekly report",
        &[],
        &checkpoint("delegating"),
    )
    .await
    .expect("park the run");
    crud::insert_child_run(db, &child, run_id, "weekly report", "analytics", 0, None)
        .await
        .expect("insert child");
    crud::update_task_status(db, &child, "awaiting_input", None)
        .await
        .expect("park the child");
}

fn platform() -> Arc<dyn PlatformContext> {
    Arc::new(FakePlatform)
}

/// The startup pass, for one workspace.
pub(crate) async fn startup_pass(db: &DatabaseConnection, ws: Uuid, probe: &Arc<Probe>) {
    agentic_pipeline::recovery::recover_active_runs(
        db.clone(),
        Arc::new(RuntimeState::new()),
        platform(),
        Arc::new(IdentityResolver),
        None,
        None,
        None,
        None,
        agentic_runtime::router::noop_router(),
        Some(ws),
        Some(registry(probe)),
        DrivePolicy::ALL,
    )
    .await;
}

/// The latency worker's pass, for one workspace.
pub(crate) async fn latency_pass(db: &DatabaseConnection, ws: Uuid, probe: &Arc<Probe>) {
    agentic_pipeline::recovery::recover_pending_global_runs(
        db.clone(),
        Arc::new(RuntimeState::new()),
        platform(),
        Arc::new(IdentityResolver),
        None,
        None,
        None,
        None,
        agentic_runtime::router::noop_router(),
        Some(ws),
        Some(registry(probe)),
        DrivePolicy::ALL,
    )
    .await;
}

/// Wait until the probe has started `control`: the proof that the pass's
/// workers are claiming, so a run they have not started was left on purpose.
/// Then give a worker that was going to claim the parked run time to do it.
pub(crate) async fn wait_for_the_workers(probe: &Probe, control: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !probe.started(control) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the control run was never started: the pass's workers are not claiming"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
}

/// The parked run was not started again, still reads as parked, and its entry
/// is held by this process rather than waiting for a worker.
pub(crate) async fn assert_still_parked(
    db: &DatabaseConnection,
    probe: &Probe,
    run_id: &str,
    status: &str,
) {
    assert!(
        !probe.started(run_id),
        "a run parked at a suspension was started again from the top"
    );
    let run = crud::get_run(db, run_id).await.unwrap().expect("run");
    assert_eq!(run.task_status.as_deref(), Some(status));
    let entry = crud::get_queue_entry(db, run_id)
        .await
        .unwrap()
        .expect("queue entry");
    assert_eq!(
        entry.queue_status, "claimed",
        "the parked run's entry must be held, not left for a worker to start"
    );
    assert_eq!(
        entry.worker_id.as_deref(),
        Some(agentic_runtime::transport::process_worker_id()),
        "held by the process that recovered it"
    );
    assert_eq!(
        entry.claim_count, 0,
        "holding a parked run's entry runs nothing, so it spends no retry budget"
    );
}

/// The interactive case, and the one a queued ask reaches: parked on a person
/// when its driver died. Only the startup pass selects an `awaiting_input` root.
#[tokio::test(flavor = "multi_thread")]
async fn a_root_parked_on_a_question_is_not_started_again() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let probe = Arc::new(Probe::default());
    let control = seed(&db, ws).await;
    let parked = seed(&db, ws).await;
    park_on_a_question(&db, &parked).await;

    startup_pass(&db, ws, &probe).await;
    wait_for_the_workers(&probe, &control).await;

    assert_still_parked(&db, &probe, &parked, "awaiting_input").await;
}

/// What a scheduled agent can reach today: parked on a delegation when its
/// driver died, and picked up by the latency worker through its `queued` entry.
#[tokio::test(flavor = "multi_thread")]
async fn a_root_parked_on_its_children_is_not_started_again() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let probe = Arc::new(Probe::default());
    let control = seed(&db, ws).await;
    let parked = seed(&db, ws).await;
    park_on_a_child(&db, &parked).await;

    latency_pass(&db, ws, &probe).await;
    wait_for_the_workers(&probe, &control).await;

    assert_still_parked(&db, &probe, &parked, "delegating").await;
}

/// The other side of the same rule: when the entry *is* the root's
/// continuation — the `Resume` the coordinator assigned from the suspension the
/// root is at — the worker runs it, and the walk does not resume the root a
/// second time beside it.
///
/// A probe-kind run cannot actually be resumed (there is no pipeline behind
/// it), so both paths end in a failed run; what tells them apart is who got
/// there. The worker claiming the entry writes `worker_task_claimed`; the walk
/// resuming the root itself fails before any worker exists.
#[tokio::test(flavor = "multi_thread")]
async fn a_queued_resume_from_the_current_suspension_is_left_to_the_worker() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let probe = Arc::new(Probe::default());
    let run_id = seed(&db, ws).await;
    park_on_a_question(&db, &run_id).await;
    // The coordinator took the answer: the root is `running` again and its
    // entry carries the resume from that same checkpoint.
    crud::update_task_status(&db, &run_id, "running", None)
        .await
        .expect("resume the run");
    requeue_with(
        &db,
        &run_id,
        &TaskSpec::Resume {
            run_id: run_id.clone(),
            resume_data: checkpoint("which store?"),
            answer: "store 7".into(),
        },
    )
    .await;

    latency_pass(&db, ws, &probe).await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let claimed = crud::get_all_events(&db, &run_id)
            .await
            .unwrap()
            .iter()
            .any(|e| e.event_type == "worker_task_claimed");
        if claimed {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the queued resume was never claimed by the worker"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!probe.started(&run_id), "a resume is not a fresh start");
}
