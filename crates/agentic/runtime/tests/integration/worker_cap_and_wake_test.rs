//! The worker concurrency cap is per process, and an enqueue wakes only the
//! scoped pollers that can claim it.
//!
//! Hardening plan item 8 (`internal-docs/2026-10-13-orchestrator-hardening-plan.md`):
//! the cap used to be a semaphore per `Worker` — one per run — so a process
//! driving K runs executed up to 32×K tasks; and the `oxy_task_enqueued`
//! NOTIFY carried no task identity, so every enqueue fleet-wide sent every
//! run's poller in every process back to the queue.
//!
//! Run:
//!   cargo nextest run -p agentic-runtime --test integration -E 'test(worker_cap_and_wake_test)'

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agentic_core::delegation::{SuspendReason, TaskAssignment, TaskOutcome, TaskSpec};
use agentic_core::human_input::SuspendedRunData;
use agentic_core::transport::WorkerTransport;
use agentic_runtime::crud;
use agentic_runtime::orchestrator::transport::DurableTransport;
use agentic_runtime::router::{PostgresTaskRouter, TaskRouter, noop_router};
use agentic_runtime::worker::{ExecutingTask, TaskExecutor, Worker};
use async_trait::async_trait;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use tokio::sync::{Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

use super::eviction_safety_test::test_db;

/// Generous: the machine running these is heavily loaded.
const SETTLE: Duration = Duration::from_secs(30);

fn agent_spec() -> TaskSpec {
    TaskSpec::Agent {
        agent_id: "test-agent".into(),
        question: "q".into(),
        extra: None,
    }
}

async fn enqueue(db: &DatabaseConnection, run_id: &str, task_id: &str) {
    crud::enqueue_task(
        db,
        task_id,
        run_id,
        None,
        &agent_spec(),
        None,
        crud::TaskScope::Scoped,
    )
    .await
    .unwrap();
}

/// A run row plus `n` queued tasks under it (`<run>.1` … `<run>.n`).
async fn seed_run(db: &DatabaseConnection, prefix: &str, n: usize) -> String {
    let run_id = format!("{prefix}-{}", uuid::Uuid::new_v4());
    crud::insert_run(db, &run_id, "q", None, "analytics", None, uuid::Uuid::nil())
        .await
        .unwrap();
    for i in 1..=n {
        enqueue(db, &run_id, &format!("{run_id}.{i}")).await;
    }
    run_id
}

async fn claimed_under(db: &DatabaseConnection, roots: &[&str]) -> i64 {
    let mut total = 0;
    for root in roots {
        let row = db
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT count(*)::bigint AS n FROM agentic_task_queue \
                 WHERE queue_status = 'claimed' AND task_id LIKE $1 || '.%'",
                [(*root).into()],
            ))
            .await
            .unwrap()
            .unwrap();
        total += row.try_get::<i64>("", "n").unwrap();
    }
    total
}

/// Parks every task until the test hands out a release. Records how many
/// run at once, the highest that ever did, and the order tasks started in.
struct Probe {
    running: AtomicUsize,
    peak: AtomicUsize,
    started: Mutex<Vec<String>>,
    release: Semaphore,
    /// Task ids ending `.s` report `Suspended` and keep their outcome channel
    /// open until this fires — a pipeline waiting to be resumed in place.
    hold_suspended: CancellationToken,
}

impl Default for Probe {
    fn default() -> Self {
        Self {
            running: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            started: Mutex::new(Vec::new()),
            release: Semaphore::new(0),
            hold_suspended: CancellationToken::new(),
        }
    }
}

impl Probe {
    fn started(&self) -> Vec<String> {
        self.started.lock().unwrap().clone()
    }

    async fn wait_started(&self, n: usize) -> bool {
        tokio::time::timeout(SETTLE, async {
            while self.started.lock().unwrap().len() < n {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .is_ok()
    }
}

struct ProbeExecutor(Arc<Probe>);

fn suspended() -> TaskOutcome {
    TaskOutcome::Suspended {
        reason: SuspendReason::HumanInput { questions: vec![] },
        resume_data: SuspendedRunData {
            from_state: "clarifying".into(),
            original_input: "q".into(),
            trace_id: "t".into(),
            stage_data: serde_json::json!({}),
            question: "?".into(),
            suggestions: vec![],
        },
        trace_id: "t".into(),
    }
}

#[async_trait]
impl TaskExecutor for ProbeExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let probe = Arc::clone(&self.0);
        let now = probe.running.fetch_add(1, Ordering::SeqCst) + 1;
        probe.peak.fetch_max(now, Ordering::SeqCst);
        probe
            .started
            .lock()
            .unwrap()
            .push(assignment.task_id.clone());
        let (_event_tx, events) = mpsc::channel(1);
        let (outcome_tx, outcomes) = mpsc::channel(4);
        tokio::spawn(async move {
            if assignment.task_id.ends_with(".s") {
                let _ = outcome_tx.send(suspended()).await;
                probe.running.fetch_sub(1, Ordering::SeqCst);
                probe.hold_suspended.cancelled().await;
                return;
            }
            probe.release.acquire().await.unwrap().forget();
            probe.running.fetch_sub(1, Ordering::SeqCst);
            let done = TaskOutcome::Done {
                answer: "ok".into(),
                metadata: None,
            };
            let _ = outcome_tx.send(done).await;
        });
        Ok(ExecutingTask {
            events,
            outcomes,
            cancel: CancellationToken::new(),
            answers: None,
        })
    }
}

/// Start a worker scoped to `root`, sharing `slots`. Returns its transport so
/// the test can retire it.
fn start_worker(
    db: &DatabaseConnection,
    root: &str,
    probe: &Arc<Probe>,
    slots: &Arc<Semaphore>,
) -> Arc<DurableTransport> {
    let transport = DurableTransport::with_router(db.clone(), noop_router(), Some(root.into()));
    let worker = Worker::with_slots(
        transport.clone() as Arc<dyn WorkerTransport>,
        Arc::new(ProbeExecutor(Arc::clone(probe))),
        Arc::clone(slots),
    );
    tokio::spawn(async move { worker.run().await });
    transport
}

/// Two runs on one process share one cap: with a cap of 2 and three tasks
/// in each run, never more than two execute and never more than two rows are
/// claimed — a row is not claimed while no slot is free. And the second run
/// is not starved behind the first: it starts before the first finishes.
#[tokio::test]
async fn two_runs_share_one_process_cap() {
    let db = test_db().await.expect("test db");
    let a = seed_run(&db, "cap-a", 3).await;
    let b = seed_run(&db, "cap-b", 3).await;
    let probe = Arc::new(Probe::default());
    let slots = Arc::new(Semaphore::new(2));
    let ta = start_worker(&db, &a, &probe, &slots);
    assert!(probe.wait_started(2).await, "first two never started");
    let tb = start_worker(&db, &b, &probe, &slots);

    // Both workers are now asking for more; give them every chance to.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        probe.running.load(Ordering::SeqCst),
        2,
        "cap is 2 across both runs"
    );
    assert_eq!(
        claimed_under(&db, &[&a, &b]).await,
        2,
        "a row was claimed with no free slot: {:?}",
        probe.started()
    );

    for n in 3..=6 {
        probe.release.add_permits(1);
        assert!(probe.wait_started(n).await, "task {n} never started");
    }
    probe.release.add_permits(2);
    assert!(
        probe.peak.load(Ordering::SeqCst) <= 2,
        "peak {}",
        probe.peak.load(Ordering::SeqCst)
    );
    let first_four = probe.started()[..4].to_vec();
    assert!(
        first_four.iter().any(|t| t.starts_with(&b))
            && first_four.iter().any(|t| t.starts_with(&a)),
        "one run monopolised the cap: {first_four:?}"
    );
    ta.retire_worker_loop();
    tb.retire_worker_loop();
}

/// A run with nothing to claim holds no slot: with a cap of 1, an idle run's
/// worker started first must not keep a second run's task from executing.
#[tokio::test]
async fn an_idle_run_holds_no_slot() {
    let db = test_db().await.expect("test db");
    let idle = seed_run(&db, "cap-idle", 0).await;
    let probe = Arc::new(Probe::default());
    let slots = Arc::new(Semaphore::new(1));
    let t_idle = start_worker(&db, &idle, &probe, &slots);
    tokio::time::sleep(Duration::from_secs(1)).await;

    let busy = seed_run(&db, "cap-busy", 1).await;
    let t_busy = start_worker(&db, &busy, &probe, &slots);
    assert!(
        probe.wait_started(1).await,
        "the idle run's worker kept the only slot"
    );
    probe.release.add_permits(1);
    t_idle.retire_worker_loop();
    t_busy.retire_worker_loop();
}

/// A suspended task stops occupying its slot: with a cap of 1, a task parked
/// on a person must not keep another run's task from executing.
#[tokio::test]
async fn a_suspended_task_gives_its_slot_back() {
    let db = test_db().await.expect("test db");
    let parked = seed_run(&db, "cap-parked", 0).await;
    enqueue(&db, &parked, &format!("{parked}.s")).await;
    let probe = Arc::new(Probe::default());
    let slots = Arc::new(Semaphore::new(1));
    let t_parked = start_worker(&db, &parked, &probe, &slots);
    assert!(
        probe.wait_started(1).await,
        "the suspending task never started"
    );

    let other = seed_run(&db, "cap-other", 1).await;
    let t_other = start_worker(&db, &other, &probe, &slots);
    assert!(
        probe.wait_started(2).await,
        "a suspended task kept the only slot"
    );
    probe.release.add_permits(1);
    probe.hold_suspended.cancel();
    t_parked.retire_worker_loop();
    t_other.retire_worker_loop();
}

// ── Wakes ────────────────────────────────────────────────────────────────────

const WAIT: Duration = Duration::from_secs(4);
/// Anything under this came from a notification, not the `WAIT` backstop.
const WOKEN: Duration = Duration::from_secs(2);

async fn started_router(
    url: &str,
    db: &DatabaseConnection,
) -> (Arc<PostgresTaskRouter>, CancellationToken) {
    let factory = PostgresTaskRouter::password_factory_from_url(url).expect("valid url");
    let started = PostgresTaskRouter::start(db.clone(), factory);
    // Let the listener connect and LISTEN (its connect wake lands here too).
    tokio::time::sleep(Duration::from_secs(1)).await;
    started
}

/// Park a waiter scoped to `root`, run `fire`, and return how long it waited.
async fn wake_time<F, Fut>(
    router: &Arc<PostgresTaskRouter>,
    root: Option<String>,
    fire: F,
) -> Duration
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let r = Arc::clone(router);
    let waiter = tokio::spawn(async move {
        let start = tokio::time::Instant::now();
        r.wait_for_task_under(root.as_deref(), WAIT).await;
        start.elapsed()
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    fire().await;
    waiter.await.unwrap()
}

/// An enqueue under root A does not wake a poller scoped to root B.
///
/// Retried: the test database is shared, and a peer test's untagged NOTIFY
/// legitimately wakes every waiter. A router that ignores the root wakes on
/// every attempt, because every attempt enqueues under A.
#[tokio::test]
async fn an_enqueue_for_another_root_does_not_wake_a_scoped_poller() {
    let (url, db) = super::router_test::test_db().await.expect("test db");
    let (router, cancel) = started_router(&url, &db).await;
    let b = seed_run(&db, "wake-b", 0).await;
    let mut waits = Vec::new();
    for _ in 0..3 {
        let a = seed_run(&db, "wake-a", 0).await;
        let waited = wake_time(&router, Some(b.clone()), || enqueue(&db, &a, &a)).await;
        waits.push(waited);
        if waited >= WAIT - Duration::from_millis(100) {
            cancel.cancel();
            return;
        }
    }
    cancel.cancel();
    panic!("B's poller woke for A's enqueue on every attempt: {waits:?}");
}

/// An enqueue of a child under root B wakes B's scoped poller.
#[tokio::test]
async fn an_enqueue_under_its_root_wakes_a_scoped_poller() {
    let (url, db) = super::router_test::test_db().await.expect("test db");
    let (router, cancel) = started_router(&url, &db).await;
    let b = seed_run(&db, "wake-own", 0).await;
    let child = format!("{b}.1");
    let waited = wake_time(&router, Some(b.clone()), || enqueue(&db, &b, &child)).await;
    assert!(
        waited < WOKEN,
        "B's poller missed its own child: {waited:?}"
    );
    cancel.cancel();
}

/// A NOTIFY a router cannot attribute — the empty payload an older pod's
/// trigger sends, or anything unparseable — wakes every scoped poller.
#[tokio::test]
async fn an_unattributable_notify_wakes_a_scoped_poller() {
    let (url, db) = super::router_test::test_db().await.expect("test db");
    let (router, cancel) = started_router(&url, &db).await;
    let b = format!("wake-legacy-{}", uuid::Uuid::new_v4());
    for payload in ["", "io_bound", "task:"] {
        let waited = wake_time(&router, Some(b.clone()), || async {
            db.execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT pg_notify('oxy_task_enqueued', $1)",
                [payload.into()],
            ))
            .await
            .unwrap();
        })
        .await;
        assert!(
            waited < WOKEN,
            "payload {payload:?} did not wake: {waited:?}"
        );
    }
    cancel.cancel();
}

/// An unscoped poller wakes for an enqueue under any root.
#[tokio::test]
async fn an_unscoped_poller_wakes_on_any_enqueue() {
    let (url, db) = super::router_test::test_db().await.expect("test db");
    let (router, cancel) = started_router(&url, &db).await;
    let a = seed_run(&db, "wake-any", 0).await;
    let waited = wake_time(&router, None, || enqueue(&db, &a, &a)).await;
    assert!(waited < WOKEN, "the unscoped poller missed it: {waited:?}");
    cancel.cancel();
}
