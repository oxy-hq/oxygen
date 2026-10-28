//! A queued analytics run: what the seed writes, and what a driver holds once
//! the run stops to ask its user a question.
//!
//! The run is seeded as a request seeds it (`queued_run::seed_queued_analytics_run`)
//! and driven by a latency-worker pass (`recover_pending_global_runs`) with a
//! stand-in executor registered for its kind — one that asks a question, or
//! answers — since no LLM is reachable from a test. What the pass does around
//! the executor is production's: the lease, the claim, the coordinator, and
//! what happens when the root parks on a person.
//!
//! Run:
//!   cargo nextest run -p agentic-pipeline --test integration -E 'test(queued_analytics_park_test)'

use std::sync::Arc;
use std::time::Duration;

use agentic_core::delegation::{SuspendReason, TaskAssignment, TaskOutcome, TaskSpec};
use agentic_core::human_input::SuspendedRunData;
use agentic_pipeline::platform::IdentityResolver;
use agentic_pipeline::queued_answer::{QueuedAnswer, close_if_undriven, queue_answer};
use agentic_pipeline::queued_run::{QueuedAnalyticsRun, seed_queued_analytics_run};
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

const KIND: &str = "queued_analytics_park_test";

/// Asks its user which store, as an analytics pipeline does from Clarifying.
struct AsksAQuestion;

#[async_trait]
impl TaskExecutor for AsksAQuestion {
    async fn execute(&self, _assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let (event_tx, events) = mpsc::channel(4);
        let (outcome_tx, outcomes) = mpsc::channel(1);
        let _ = event_tx
            .send((
                "awaiting_input".into(),
                json!({ "questions": [{ "prompt": "which store?", "suggestions": [] }] }),
            ))
            .await;
        let _ = outcome_tx
            .send(TaskOutcome::Suspended {
                reason: SuspendReason::HumanInput {
                    questions: vec![agentic_core::HumanInputQuestion {
                        prompt: "which store?".into(),
                        suggestions: vec![],
                    }],
                },
                resume_data: SuspendedRunData {
                    from_state: "clarifying".into(),
                    original_input: "how did the stores do?".into(),
                    trace_id: "trace-1".into(),
                    stage_data: json!({}),
                    question: "which store?".into(),
                    suggestions: vec![],
                },
                trace_id: "trace-1".into(),
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

fn seed(ws: Uuid) -> QueuedAnalyticsRun {
    QueuedAnalyticsRun {
        run_id: Uuid::new_v4().to_string(),
        workspace_id: ws,
        agent_id: "sales".into(),
        question: "how did the stores do?".into(),
        thread_id: None,
        thinking_mode: Default::default(),
        metadata: [
            ("who".to_string(), json!("someone")),
            // A host key never replaces one the pipeline writes.
            ("agent_id".to_string(), json!("not-this-one")),
        ]
        .into_iter()
        .collect(),
        spec: TaskSpec::Custom {
            kind: KIND.into(),
            payload: json!({}),
        },
    }
}

async fn latency_pass(db: &DatabaseConnection, ws: Uuid, state: &Arc<RuntimeState>) {
    let mut registry = CustomTaskRegistry::new();
    registry.register(KIND, Arc::new(AsksAQuestion));
    agentic_pipeline::recovery::recover_pending_global_runs(
        db.clone(),
        state.clone(),
        Arc::new(FakePlatform),
        Arc::new(IdentityResolver),
        None,
        None,
        None,
        None,
        agentic_runtime::router::noop_router(),
        Some(ws),
        Some(Arc::new(registry)),
        DrivePolicy::ALL,
    )
    .await;
}

/// Wait until `check` holds of the run and its queue row, or fail naming what
/// was last seen.
async fn until(
    db: &DatabaseConnection,
    run_id: &str,
    what: &str,
    check: impl Fn(
        &agentic_runtime::entity::run::Model,
        Option<&agentic_runtime::entity::task_queue::Model>,
    ) -> bool,
) -> (
    agentic_runtime::entity::run::Model,
    Option<agentic_runtime::entity::task_queue::Model>,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let run = crud::get_run(db, run_id).await.unwrap().expect("run");
        let entry = crud::get_queue_entry(db, run_id).await.unwrap();
        if check(&run, entry.as_ref()) {
            return (run, entry);
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what}: run {:?} driver {:?}, entry {:?}",
            run.task_status,
            run.driver_id,
            entry.map(|e| e.queue_status)
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Three rows, one transaction: the run with the pipeline's own metadata and
/// the host's beside it, its extension row, and one `queued` Global entry.
#[tokio::test(flavor = "multi_thread")]
async fn the_seed_writes_the_run_its_extension_and_its_entry() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let run = seed(ws);

    seed_queued_analytics_run(&db, &run).await.expect("seed");

    let row = crud::get_run(&db, &run.run_id).await.unwrap().expect("run");
    assert_eq!(row.source_type.as_deref(), Some("analytics"));
    assert_eq!(row.task_status.as_deref(), Some("running"));
    assert_eq!(row.workspace_id, ws);
    let metadata = row.metadata.expect("metadata");
    assert_eq!(
        metadata["agent_id"],
        json!("sales"),
        "the pipeline's key wins"
    );
    assert_eq!(metadata["thinking_mode"], json!("auto"));
    assert_eq!(metadata["who"], json!("someone"));
    assert!(
        agentic_pipeline::run_execution::get_run_execution(&db, &run.run_id)
            .await
            .unwrap()
            .is_some(),
        "the extension row is where a driver takes the execution stamp"
    );
    let entry = crud::get_queue_entry(&db, &run.run_id)
        .await
        .unwrap()
        .expect("entry");
    assert_eq!(entry.queue_status, "queued");
    assert!(!entry.scope_owned, "Global: any driver may take it");
    assert_eq!(entry.spec, serde_json::to_value(&run.spec).unwrap());
}

/// The root asks its user a question. The run is left as its suspension
/// describes it — `awaiting_input`, checkpoint on record — and the driver
/// holds nothing for it: the claim is ended, not left `claimed` under a
/// heartbeat, and the driver lease is released. A cancel from any replica then
/// closes the run, where a held claim would have read as "a driver has it"
/// for as long as that process lived.
#[tokio::test(flavor = "multi_thread")]
async fn a_root_that_asks_its_user_a_question_holds_nothing_while_it_waits() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let run = seed(ws);
    seed_queued_analytics_run(&db, &run).await.expect("seed");
    let state = Arc::new(RuntimeState::new());

    latency_pass(&db, ws, &state).await;
    let (row, entry) = until(&db, &run.run_id, "the run never parked", |run, entry| {
        run.task_status.as_deref() == Some("awaiting_input")
            && run.driver_id.is_none()
            && entry.is_some_and(|e| e.queue_status == "completed")
    })
    .await;

    assert!(
        crud::get_suspension(&db, &run.run_id)
            .await
            .unwrap()
            .is_some(),
        "the checkpoint an answer continues from"
    );
    assert_eq!(row.driver_heartbeat_at, None);
    let entry = entry.expect("entry");
    assert_eq!(entry.queue_status, "completed");
    // Nothing selects it again: a second pass finds no pending Global run.
    assert!(
        crud::find_pending_global_runs(&db, Some(ws))
            .await
            .unwrap()
            .is_empty()
    );
    // And it stays as it is: no lease taken back, no heartbeat on the row.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let later = crud::get_run(&db, &run.run_id).await.unwrap().unwrap();
    assert_eq!(later.driver_id, None);
    assert_eq!(later.task_status.as_deref(), Some("awaiting_input"));

    crud::request_cancel(&db, &run.run_id).await.unwrap();
    assert!(
        crud::fail_undriven_run(&db, &run.run_id, "cancelled by user")
            .await
            .unwrap(),
        "nothing drives a parked run, so a cancel closes it"
    );
}

/// Seed a run, let a latency pass drive it to the question, and wait until it
/// is parked: `awaiting_input`, no driver, its claim ended.
async fn parked(db: &DatabaseConnection, ws: Uuid) -> String {
    let run = seed(ws);
    seed_queued_analytics_run(db, &run).await.expect("seed");
    latency_pass(db, ws, &Arc::new(RuntimeState::new())).await;
    until(db, &run.run_id, "the run never parked", |run, entry| {
        run.task_status.as_deref() == Some("awaiting_input")
            && run.driver_id.is_none()
            && entry.is_some_and(|e| e.queue_status == "completed")
    })
    .await;
    run.run_id
}

/// An answer from any replica reaches a parked run through the queue: the run
/// is taken back to `running`, the answer is on its log as `input_resolved`,
/// and its own entry is the resume from the checkpoint it is at, carrying the
/// answer. A second answer finds nothing waiting.
#[tokio::test(flavor = "multi_thread")]
async fn an_answer_to_a_parked_run_queues_its_continuation_once() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let run_id = parked(&db, ws).await;
    let checkpoint = crud::get_suspension(&db, &run_id).await.unwrap().unwrap();

    assert_eq!(
        queue_answer(&db, &run_id, "store 7").await.unwrap(),
        QueuedAnswer::Queued
    );

    let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(run.task_status.as_deref(), Some("running"));
    let entry = crud::get_queue_entry(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(entry.queue_status, "queued");
    assert!(!entry.scope_owned);
    assert_eq!(
        entry.spec,
        serde_json::to_value(TaskSpec::Resume {
            run_id: run_id.clone(),
            resume_data: checkpoint,
            answer: "store 7".into(),
        })
        .unwrap()
    );
    let events = crud::get_all_events(&db, &run_id).await.unwrap();
    let resolved = events
        .iter()
        .find(|e| e.event_type == "input_resolved")
        .expect("the answer is on the log");
    assert_eq!(resolved.payload["answer"], json!("store 7"));
    assert_eq!(resolved.payload["trace_id"], json!("trace-1"));

    assert_eq!(
        queue_answer(&db, &run_id, "store 8").await.unwrap(),
        QueuedAnswer::NotWaiting(Some("running".into())),
        "the first answer continues the run; the second finds nothing waiting"
    );
    assert_eq!(
        crud::get_all_events(&db, &run_id)
            .await
            .unwrap()
            .iter()
            .filter(|e| e.event_type == "input_resolved")
            .count(),
        1
    );
}

/// The queued continuation is driven as a continuation, not a recovery: it
/// spends none of the run's recovery budget — or a chat would be retired by
/// its fifth clarifying question — and nothing on its log is deleted or
/// announced as a restart. (The stand-in platform has no agent, so the resume
/// itself fails; that it was attempted from the queue is what is shown.)
#[tokio::test(flavor = "multi_thread")]
async fn a_queued_answer_is_driven_without_spending_recovery_budget() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let run_id = parked(&db, ws).await;
    let before = crud::get_run(&db, &run_id).await.unwrap().unwrap().attempt;
    queue_answer(&db, &run_id, "store 7").await.unwrap();

    latency_pass(&db, ws, &Arc::new(RuntimeState::new())).await;
    let (run, entry) = until(
        &db,
        &run_id,
        "the continuation was never driven",
        |run, _| matches!(run.task_status.as_deref(), Some("failed" | "done")),
    )
    .await;

    assert_eq!(run.attempt, before, "an answer is not a recovery attempt");
    assert_ne!(
        entry.map(|e| e.queue_status).as_deref(),
        Some("queued"),
        "the resume entry was claimed"
    );
    let events = crud::get_all_events(&db, &run_id).await.unwrap();
    let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
    assert!(types.contains(&"awaiting_input"), "{types:?}");
    assert!(types.contains(&"input_resolved"), "{types:?}");
    assert!(!types.contains(&"recovery_resumed"), "{types:?}");
}

/// A run a live driver still holds is not continued under it, and a run with
/// no checkpoint has nothing to continue from.
#[tokio::test(flavor = "multi_thread")]
async fn an_answer_is_not_queued_under_a_live_driver_or_without_a_checkpoint() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let run_id = parked(&db, ws).await;
    assert!(
        crud::try_acquire_driver(&db, &run_id, "another-pod")
            .await
            .unwrap()
    );
    assert_eq!(
        queue_answer(&db, &run_id, "store 7").await.unwrap(),
        QueuedAnswer::DrivenElsewhere
    );
    let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(run.task_status.as_deref(), Some("awaiting_input"));

    let fresh = seed(ws);
    seed_queued_analytics_run(&db, &fresh).await.expect("seed");
    assert_eq!(
        queue_answer(&db, &fresh.run_id, "store 7").await.unwrap(),
        QueuedAnswer::NoCheckpoint
    );
}

/// The cancel half: a run with a queue entry someone holds or is about to
/// take is left to that driver; a parked run nothing drives is closed.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_closes_only_a_run_nothing_can_be_driving() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let queued = seed(ws);
    seed_queued_analytics_run(&db, &queued).await.expect("seed");
    assert!(
        !close_if_undriven(&db, &queued.run_id, "cancelled by user")
            .await
            .unwrap(),
        "a queued run is its driver's to close"
    );

    let run_id = parked(&db, ws).await;
    assert!(
        close_if_undriven(&db, &run_id, "cancelled by user")
            .await
            .unwrap()
    );
    let run = crud::get_run(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(run.task_status.as_deref(), Some("failed"));
    assert!(
        !close_if_undriven(&db, &run_id, "cancelled by user")
            .await
            .unwrap(),
        "an ended run stays as it ended"
    );
}

async fn set_entry(db: &DatabaseConnection, run_id: &str, assignment: &str) {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!("UPDATE agentic_task_queue SET {assignment} WHERE task_id = $1"),
        [run_id.into()],
    ))
    .await
    .expect("update the queue row");
}

/// A root answered in process before has a scoped row: its coordinator's
/// transport assigned that resume `Scoped`. The enqueue's upsert keeps an
/// existing row's scope, so an answer queued over it must make it `Global`
/// again — a scoped row is invisible to every latency worker.
#[tokio::test(flavor = "multi_thread")]
async fn an_answer_over_a_scoped_row_queues_a_continuation_any_driver_selects() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let run_id = parked(&db, ws).await;
    set_entry(&db, &run_id, "scope_owned = true").await;

    assert_eq!(
        queue_answer(&db, &run_id, "store 7").await.unwrap(),
        QueuedAnswer::Queued
    );

    let entry = crud::get_queue_entry(&db, &run_id).await.unwrap().unwrap();
    assert!(!entry.scope_owned);
    assert_eq!(
        crud::find_pending_global_runs(&db, Some(ws))
            .await
            .unwrap()
            .iter()
            .map(|r| r.run_id.as_str())
            .collect::<Vec<_>>(),
        vec![run_id.as_str()]
    );
}

/// A continuation a driver started and lost is requeued with the same spec —
/// by the reaper after a crash (its claim charged), or by a graceful release
/// on a rolling deploy (its claim refunded to zero). Either way the driver
/// wrote past the answer before it went, and the run is recovered as an
/// interrupted one — a unit of budget spent, the log's tail cleaned up and
/// marked — not driven as a fresh continuation.
#[tokio::test(flavor = "multi_thread")]
async fn an_interrupted_continuation_is_recovered_not_continued() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    for released in ["claim_count = 1", "claim_count = 0"] {
        let ws = Uuid::new_v4();
        let run_id = parked(&db, ws).await;
        queue_answer(&db, &run_id, "store 7").await.unwrap();
        // The driver that claimed the resume got this far.
        let seq = crud::get_max_seq(&db, &run_id).await.unwrap() + 1;
        crud::insert_event(&db, &run_id, seq, "step_started", &json!({}), 0)
            .await
            .unwrap();
        set_entry(&db, &run_id, released).await;
        let before = crud::get_run(&db, &run_id).await.unwrap().unwrap().attempt;

        latency_pass(&db, ws, &Arc::new(RuntimeState::new())).await;
        let (run, _) = until(
            &db,
            &run_id,
            "the continuation was never recovered",
            |run, _| {
                run.attempt > before
                    || matches!(run.task_status.as_deref(), Some("failed" | "done"))
            },
        )
        .await;

        assert_eq!(
            run.attempt,
            before + 1,
            "{released}: a recovery spends budget"
        );
        let types: Vec<String> = crud::get_all_events(&db, &run_id)
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.event_type)
            .collect();
        assert!(
            types.iter().any(|t| t == "recovery_resumed"),
            "{released}: {types:?}"
        );
    }
}
