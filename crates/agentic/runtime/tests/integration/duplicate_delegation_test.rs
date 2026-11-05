//! A task that is already waiting on children must never spawn a second set.
//!
//! Regression cover for the bug where one `type: airway` step in a workflow
//! produced up to three concurrent executions of the same pipeline. The chain:
//! a delegating step suspends, `DurableTransport` deliberately writes nothing
//! (the row must stay `claimed` so it can resume) — but suspending also
//! completes the driver future, which cancelled the heartbeat ticker and froze
//! `last_heartbeat`. The reaper read that as a dead worker and re-queued the
//! task at the visibility timeout; a worker re-claimed it, the decider re-ran,
//! the child still hadn't answered, and it delegated the SAME step again. Up to
//! `max_claims` copies, then dead-lettered.
//!
//! The ticker half is fixed in `Worker::handle_task` and covered by
//! `heartbeat_lifetime_tests` there — a unit test, because shrinking
//! `visibility_timeout_secs` below the 15s heartbeat interval to make an
//! end-to-end version fast would just make the reaper win for an unrelated
//! reason.
//!
//! This is the second half: even if something *does* re-drive a suspended
//! coordinator (crash recovery re-running a decider, a future re-claim path),
//! the coordinator must recognise the re-delegation and drop it rather than
//! minting another `{task_id}.{n}`.
//!
//! Run:
//!   cargo nextest run -p agentic-runtime --test integration -E 'test(duplicate_delegation_test)'

use agentic_core::delegation::{
    DelegationTarget, SuspendReason, TaskAssignment, TaskOutcome, TaskSpec,
};
use agentic_core::human_input::SuspendedRunData;
use agentic_core::transport::{CoordinatorTransport, WorkerTransport};
use agentic_runtime::coordinator::Coordinator;
use agentic_runtime::crud;
use agentic_runtime::migration::RuntimeMigrator;
use agentic_runtime::state::RuntimeState;
use agentic_runtime::transport::LocalTransport;
use agentic_runtime::worker::{ExecutingTask, TaskExecutor, Worker};
use async_trait::async_trait;
use sea_orm::{Database, DatabaseConnection};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

static TEST_DB_URL: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();
static TEST_CONTAINER: tokio::sync::OnceCell<
    std::sync::Arc<testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>>,
> = tokio::sync::OnceCell::const_new();

async fn test_db() -> Option<DatabaseConnection> {
    let url = TEST_DB_URL
        .get_or_init(|| async {
            if let Ok(url) = std::env::var("OXY_DATABASE_URL") {
                return url;
            }
            use testcontainers::runners::AsyncRunner;
            use testcontainers::{ImageExt, ReuseDirective};
            use testcontainers_modules::postgres::Postgres;
            let container = TEST_CONTAINER
                .get_or_init(|| async {
                    std::sync::Arc::new(
                        Postgres::default()
                            .with_tag("18-alpine")
                            .with_reuse(ReuseDirective::Always)
                            .start()
                            .await
                            .expect("failed to start Postgres testcontainer"),
                    )
                })
                .await;
            let port = container.get_host_port_ipv4(5432_u16).await.unwrap();
            format!("postgresql://postgres:postgres@127.0.0.1:{port}/postgres")
        })
        .await
        .clone();

    let db = Database::connect(&url)
        .await
        .expect("failed to connect to test database");
    oxy_test_utils::migration::migrate_shared_test_db::<RuntimeMigrator>(&url, &db)
        .await
        .expect("shared migrations failed")
        .finish()
        .await;
    Some(db)
}

fn suspend_data() -> SuspendedRunData {
    SuspendedRunData {
        from_state: "executing".into(),
        original_input: "ingest and roll up".into(),
        trace_id: "trace-1".into(),
        stage_data: json!({}),
        question: "run the pipeline".into(),
        suggestions: vec![],
    }
}

fn delegation() -> SuspendReason {
    SuspendReason::Delegation {
        target: DelegationTarget::Automation {
            workflow_ref: "pipelines/restaurant_analytics.airway.yml".into(),
        },
        request: "run pipeline".into(),
        context: json!({}),
        policy: None,
    }
}

/// Stands in for a decider that gets re-driven while its first delegation is
/// still outstanding: it reports `Suspended { Delegation }` **twice** for the
/// same task, back to back.
///
/// That is exactly the message sequence a reaper-induced re-claim produces —
/// two `Suspended` outcomes for one `task_id` with no resume in between. Both
/// land in the shared worker→coordinator channel before the child is even
/// assigned, so the coordinator provably sees the duplicate while still
/// `WaitingOnChildren`; the child's `Done` is queued behind them.
struct ReDelegatingExecutor;

#[async_trait]
impl TaskExecutor for ReDelegatingExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let (event_tx, event_rx) = mpsc::channel(16);
        let (outcome_tx, outcome_rx) = mpsc::channel(4);
        let spec = assignment.spec.clone();

        tokio::spawn(async move {
            drop(event_tx);
            match spec {
                TaskSpec::Agent { .. } => {
                    for _ in 0..2 {
                        let _ = outcome_tx
                            .send(TaskOutcome::Suspended {
                                reason: delegation(),
                                resume_data: suspend_data(),
                                trace_id: "trace-1".into(),
                            })
                            .await;
                    }
                }
                // The delegated child — the "airway step". Slow enough that a
                // second delegation would overlap it, which is the defect.
                TaskSpec::Automation { .. } => {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    let _ = outcome_tx
                        .send(TaskOutcome::Done {
                            answer: "pipeline done".into(),
                            metadata: None,
                        })
                        .await;
                }
                TaskSpec::Resume { .. } => {
                    let _ = outcome_tx
                        .send(TaskOutcome::Done {
                            answer: "resumed".into(),
                            metadata: None,
                        })
                        .await;
                }
                other => panic!("unexpected spec in this test: {other:?}"),
            }
        });

        Ok(ExecutingTask {
            events: event_rx,
            outcomes: outcome_rx,
            cancel: CancellationToken::new(),
            answers: None,
        })
    }
}

#[tokio::test]
async fn re_delegation_while_waiting_on_children_spawns_no_second_child() {
    let Some(db) = test_db().await else {
        return;
    };
    let run_id = format!("dup-deleg-{}", uuid::Uuid::new_v4());
    crud::insert_run(
        &db,
        &run_id,
        "ingest and roll up",
        None,
        "workflow",
        None,
        uuid::Uuid::nil(),
    )
    .await
    .unwrap();

    let state = Arc::new(RuntimeState::new());
    let transport = LocalTransport::with_defaults();
    let worker = Worker::new(
        transport.clone() as Arc<dyn WorkerTransport>,
        Arc::new(ReDelegatingExecutor),
    );

    let (_answer_tx, answer_rx) = mpsc::channel::<String>(1);
    let mut coordinator = Coordinator::new(
        db.clone(),
        state.clone(),
        transport.clone() as Arc<dyn CoordinatorTransport>,
    );
    coordinator.register_answer_channel(run_id.clone(), answer_rx);
    coordinator
        .submit_root(
            run_id.clone(),
            TaskSpec::Agent {
                agent_id: "automation".into(),
                question: "ingest and roll up".into(),
                extra: None,
            },
        )
        .await
        .unwrap();

    tokio::spawn(async move { worker.run().await });
    let coord_handle = tokio::spawn(async move { coordinator.run().await });
    tokio::time::timeout(Duration::from_secs(30), coord_handle)
        .await
        .expect("coordinator timed out")
        .expect("coordinator panicked");

    // One delegation, one child — the duplicate is dropped, not honoured.
    let tree = crud::load_task_tree(&db, &run_id).await.unwrap();
    let children: Vec<&str> = tree
        .iter()
        .filter(|r| r.id != run_id)
        .map(|r| r.id.as_str())
        .collect();
    assert_eq!(
        children.len(),
        1,
        "one delegated step must produce exactly one child run; got {children:?} \
         (each extra child is another concurrent execution of the same step)"
    );
    assert_eq!(children[0], format!("{run_id}.1"));

    // And the duplicate must not have stranded the parent: the original child
    // still resumes it, so the run reaches a terminal state normally.
    let root = crud::get_run(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(root.task_status.as_deref(), Some("done"));
}

/// At most one heartbeat ticker per `task_id`, and it is the newest claim's.
///
/// Keeping the ticker alive across a suspension means the resume path can hand
/// the same `task_id` to a second driver — a new claim — while the first
/// driver's ticker is still running. The heartbeat predicate is fenced on the
/// claim's epoch so the stale ticker could no longer keep the new claim
/// looking alive; this registry is what ends it promptly instead of leaving
/// one ticker per suspension beating (and failing) for the rest of the run.
#[tokio::test]
async fn spawning_a_heartbeat_retires_the_previous_one_for_that_task() {
    let Some(db) = test_db().await else {
        return;
    };
    let transport = agentic_runtime::transport::DurableTransport::new(db);
    let interval = agentic_runtime::orchestrator::worker::HEARTBEAT_INTERVAL;

    let first = WorkerTransport::spawn_heartbeat(transport.as_ref(), "hb-task", 1, interval);
    assert!(
        !first.is_cancelled(),
        "a freshly spawned ticker must be live"
    );

    // The resume path re-claims the same task_id — this is the second driver.
    let second = WorkerTransport::spawn_heartbeat(transport.as_ref(), "hb-task", 2, interval);
    assert!(
        first.is_cancelled(),
        "spawning a ticker for a newer claim of a task_id that already has one \
         must retire the old one"
    );
    assert!(
        !second.is_cancelled(),
        "the current driver's ticker must survive"
    );

    // A late registration of an OLDER claim must not displace the newer one.
    let late = WorkerTransport::spawn_heartbeat(transport.as_ref(), "hb-task", 1, interval);
    assert!(
        late.is_cancelled(),
        "an older claim's ticker is born retired"
    );
    assert!(!second.is_cancelled(), "the newer claim's ticker stays");

    // A different task is unaffected — the retirement is keyed, not global.
    let other = WorkerTransport::spawn_heartbeat(transport.as_ref(), "hb-other", 1, interval);
    assert!(!second.is_cancelled(), "sibling task must not be retired");
    assert!(!other.is_cancelled());
}

/// `fail_orphaned_claim` may only terminate a claim THIS process holds.
///
/// It exists to stop an unplaceable suspension from leaving a row `claimed` on
/// a live heartbeat, and the caller supplies no `worker_id` — so without an
/// ownership fence it would also match a row a peer had since re-claimed, or
/// one sitting legitimately `queued`, and silently kill live work.
#[tokio::test]
async fn fail_orphaned_claim_only_touches_this_process_claim() {
    let Some(db) = test_db().await else {
        return;
    };
    let me = agentic_runtime::transport::process_worker_id();

    // (a) A claim this process holds — terminated.
    let mine = seed_queued_task(&db).await;
    force_claim(&db, &mine, me).await;
    let hit = crud::fail_orphaned_claim(&db, &mine).await.unwrap();
    assert_eq!(hit, 1, "our own abandoned claim must be failed");
    assert_eq!(queue_status_of(&db, &mine).await.as_deref(), Some("failed"));

    // (b) A claim a peer holds — untouched. This is the case the fence buys:
    // a late Suspended arriving after the row was reaped and re-claimed
    // elsewhere must not cancel the peer's live work.
    let peers = seed_queued_task(&db).await;
    force_claim(&db, &peers, "some-peer-worker").await;
    let hit = crud::fail_orphaned_claim(&db, &peers).await.unwrap();
    assert_eq!(hit, 0, "a peer's claim must not be terminated");
    assert_eq!(
        queue_status_of(&db, &peers).await.as_deref(),
        Some("claimed"),
        "the peer must still own its row"
    );

    // (c) A row still queued — untouched; whoever claims it next owns it.
    let pending = seed_queued_task(&db).await;
    let hit = crud::fail_orphaned_claim(&db, &pending).await.unwrap();
    assert_eq!(hit, 0, "a queued row is nobody's abandoned claim");
    assert_eq!(
        queue_status_of(&db, &pending).await.as_deref(),
        Some("queued")
    );
}

/// Seed a run plus one `queued` task, and return its id.
///
/// One id, not a `(run, task)` pair: a root task's `task_id` *is* its run id,
/// and returning the same string twice only invites the reader to wonder which
/// is which.
///
/// **`Scoped`, deliberately.** `Global` (`scope_owned = false`) is what the
/// global recovery selections pick up, across the whole table — not keyed to
/// any run. This Postgres is shared with the `agentic-pipeline` and
/// `agentic-airway` suites, so a concurrent recovery pass there could take a
/// `Global` row seeded here; [`force_claim`] would then find no `queued` row
/// and trip its `rows_affected == 1` assert, failing for reasons that have
/// nothing to do with what is under test. Scoping costs this test nothing —
/// nothing here goes through the global path — and takes the race off the
/// table. It also leaves case (c)'s
/// permanently-`queued` row unclaimable by any other suite.
async fn seed_queued_task(db: &DatabaseConnection) -> String {
    let run_id = format!("orphan-{}", uuid::Uuid::new_v4());
    crud::insert_run(db, &run_id, "Q", None, "workflow", None, uuid::Uuid::nil())
        .await
        .unwrap();
    crud::enqueue_task(
        db,
        &run_id,
        &run_id,
        None,
        &TaskSpec::Agent {
            agent_id: "a".into(),
            question: "q".into(),
            extra: None,
        },
        None,
        agentic_runtime::crud::queue::TaskScope::Scoped,
    )
    .await
    .unwrap();
    run_id
}

/// Put a row into `claimed` by `worker` directly.
///
/// Deliberately NOT `claim_task_under_root`: that path also gates on
/// `available_at <= now()`, where `available_at` was written from the *app's*
/// clock by `enqueue_task` and `now()` is the *database's*. A test Postgres
/// running a few milliseconds behind the host makes a row enqueued 1–3 ms
/// earlier briefly unclaimable, so the claim returns `None` and the test fails
/// for a reason that has nothing to do with what it asserts. (That skew
/// sensitivity is real but belongs to `enqueue_task`, not here.)
///
/// What is under test is `fail_orphaned_claim`'s ownership predicate, so the
/// claim is a fixture: set the state it needs and assert on the fence.
async fn force_claim(db: &DatabaseConnection, task_id: &str, worker: &str) {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
    let rows = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE agentic_task_queue \
             SET queue_status = 'claimed', worker_id = $2, claimed_at = now(), \
                 last_heartbeat = now(), claim_count = claim_count + 1, updated_at = now() \
             WHERE task_id = $1 AND queue_status = 'queued'",
            [task_id.into(), worker.into()],
        ))
        .await
        .expect("force_claim update")
        .rows_affected();
    assert_eq!(rows, 1, "fixture must claim exactly the seeded row");
}

async fn queue_status_of(db: &DatabaseConnection, task_id: &str) -> Option<String> {
    crud::get_queue_entry(db, task_id)
        .await
        .unwrap()
        .map(|q| q.queue_status)
}
