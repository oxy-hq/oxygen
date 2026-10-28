//! A custom-app function's declared `retries` drive real retries.
//!
//! The manifest's `retries.maxAttempts` counts the first try
//! (`sdk/typescript/src/custom-app/manifest-types.ts`); the host maps it to a
//! `TaskPolicy` with `max_retries = maxAttempts - 1`, which a schedule carries
//! in its `variables.task_policy` and every fire writes onto the queue row.
//! These tests fire a `function` schedule through the scheduler's own fire
//! arm, let the latency worker take the run as it does in production, and
//! count how many times the function was actually started.
//!
//! Run:
//!   cargo nextest run -p agentic-pipeline --test integration -E 'test(function_retries_test)'

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agentic_core::delegation::{
    BackoffStrategy, RetryPolicy, TaskAssignment, TaskOutcome, TaskPolicy,
};
use agentic_pipeline::app_function_task::APP_FUNCTION_KIND;
use agentic_pipeline::platform::IdentityResolver;
use agentic_pipeline::recovery::DrivePolicy;
use agentic_pipeline::scheduler::{ScheduleInput, create_schedule, run_schedule_now};
use agentic_runtime::crud;
use agentic_runtime::state::RuntimeState;
use agentic_runtime::worker::{CustomTaskRegistry, ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::automation_recovery_test::{FakePlatform, test_db};
use crate::scheduler_test::FakeWorkspace;

/// A function that fails its first `fail_first` starts of each run, then
/// succeeds. Counts every start, per run.
struct Flaky {
    fail_first: usize,
    starts: Mutex<HashMap<String, usize>>,
}

impl Flaky {
    fn new(fail_first: usize) -> Arc<Self> {
        Arc::new(Self {
            fail_first,
            starts: Mutex::new(HashMap::new()),
        })
    }

    fn starts(&self, run_id: &str) -> usize {
        self.starts
            .lock()
            .unwrap()
            .get(run_id)
            .copied()
            .unwrap_or(0)
    }
}

#[async_trait]
impl TaskExecutor for Flaky {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let attempt = {
            let mut starts = self.starts.lock().unwrap();
            let n = starts.entry(assignment.run_id.clone()).or_default();
            *n += 1;
            *n
        };
        let outcome = if attempt <= self.fail_first {
            TaskOutcome::Failed(format!("upstream refused (attempt {attempt})"))
        } else {
            TaskOutcome::Done {
                answer: "ok".into(),
                metadata: None,
            }
        };
        let (_event_tx, events) = mpsc::channel(1);
        let (outcome_tx, outcomes) = mpsc::channel(1);
        let _ = outcome_tx.send(outcome).await;
        Ok(ExecutingTask {
            events,
            outcomes,
            cancel: CancellationToken::new(),
            answers: None,
        })
    }
}

/// What the host builds from `retries: { maxAttempts, minTimeoutMs, maxTimeoutMs }`.
fn policy(max_attempts: u32, backoff_ms: u64) -> TaskPolicy {
    TaskPolicy {
        retry: Some(RetryPolicy {
            max_retries: max_attempts - 1,
            backoff: BackoffStrategy::Exponential {
                initial_delay_ms: backoff_ms,
                max_delay_ms: backoff_ms,
            },
            retry_on: vec![],
        }),
        fallback_targets: vec![],
    }
}

/// Create a `function` schedule carrying `policy` the way the host's reconcile
/// writes it, and fire it through the scheduler.
async fn fire_function(db: &DatabaseConnection, ws: Uuid, policy: Option<TaskPolicy>) -> String {
    let app = Uuid::new_v4();
    let s = create_schedule(
        db,
        ws,
        ScheduleInput {
            name: format!("fn:{app}/refresh"),
            target_kind: "function".into(),
            target_ref: format!("{app}/refresh"),
            question: None,
            variables: policy.map(|p| serde_json::json!({ "task_policy": p })),
            cron_expr: "0 9 * * *".into(),
            timezone: "UTC".into(),
            enabled: true,
        },
    )
    .await
    .expect("create schedule");
    run_schedule_now(db, ws, &FakeWorkspace, &s.id)
        .await
        .expect("fire the schedule")
}

/// The latency worker's pass: takes every pending Global run and drives it.
async fn latency_pass(db: &DatabaseConnection, ws: Uuid, function: &Arc<Flaky>) {
    let mut registry = CustomTaskRegistry::new();
    registry.register(APP_FUNCTION_KIND, function.clone());
    agentic_pipeline::recovery::recover_pending_global_runs(
        db.clone(),
        Arc::new(RuntimeState::new()),
        Arc::new(FakePlatform),
        Arc::new(IdentityResolver),
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

/// Wait for the run to end, then long enough that an attempt the queue still
/// owed would have started.
async fn settle(db: &DatabaseConnection, run_id: &str) -> agentic_runtime::entity::run::Model {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        let run = crud::get_run(db, run_id).await.unwrap().expect("run");
        if matches!(
            run.task_status.as_deref(),
            Some("done" | "failed" | "cancelled" | "timed_out")
        ) {
            tokio::time::sleep(Duration::from_secs(2)).await;
            return crud::get_run(db, run_id).await.unwrap().expect("run");
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "run never ended; last status {:?}",
            run.task_status
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// `maxAttempts: 3` on a function that always fails: three starts, then the
/// run fails and says how many attempts it made.
#[tokio::test(flavor = "multi_thread")]
async fn a_failing_function_is_attempted_max_attempts_times_then_fails_with_the_count() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let function = Flaky::new(usize::MAX);
    let run_id = fire_function(&db, ws, Some(policy(3, 1_000))).await;

    latency_pass(&db, ws, &function).await;
    let run = settle(&db, &run_id).await;

    assert_eq!(
        function.starts(&run_id),
        3,
        "maxAttempts: 3 means the first try and two retries"
    );
    assert_eq!(run.task_status.as_deref(), Some("failed"));
    let error = run.error_message.unwrap_or_default();
    assert!(
        error.contains("3 attempts") && error.contains("upstream refused (attempt 3)"),
        "the final failure names the attempt count and the last error: {error:?}"
    );
}

/// A function that succeeds on its second try is not tried a third time.
#[tokio::test(flavor = "multi_thread")]
async fn a_success_stops_the_retries() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let function = Flaky::new(1);
    let run_id = fire_function(&db, ws, Some(policy(4, 1_000))).await;

    latency_pass(&db, ws, &function).await;
    let run = settle(&db, &run_id).await;

    assert_eq!(function.starts(&run_id), 2, "failed once, then succeeded");
    assert_eq!(run.task_status.as_deref(), Some("done"));
}

/// No `retries` block: one attempt, and the run fails with the function's own
/// error.
#[tokio::test(flavor = "multi_thread")]
async fn without_retries_a_function_runs_once() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let function = Flaky::new(usize::MAX);
    let run_id = fire_function(&db, ws, None).await;

    latency_pass(&db, ws, &function).await;
    let run = settle(&db, &run_id).await;

    assert_eq!(function.starts(&run_id), 1);
    assert_eq!(run.task_status.as_deref(), Some("failed"));
    assert_eq!(
        run.error_message.as_deref(),
        Some("upstream refused (attempt 1)"),
        "a single attempt reports the function's error unchanged"
    );
}

/// The backoff is the queue row's `available_at`, not a sleep in the run's
/// driver: while a retry waits out a long backoff, a Stop still lands, ends
/// the run, and the waiting attempt never starts.
#[tokio::test(flavor = "multi_thread")]
async fn a_stop_during_the_backoff_ends_the_run_without_another_attempt() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let function = Flaky::new(usize::MAX);
    let run_id = fire_function(&db, ws, Some(policy(3, 120_000))).await;

    latency_pass(&db, ws, &function).await;

    // The first attempt fails and its retry is parked behind the backoff.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let entry = crud::get_queue_entry(&db, &run_id).await.unwrap();
        let waiting = entry.as_ref().is_some_and(|e| {
            e.queue_status == "queued"
                && e.available_at > crud::now() + chrono::Duration::seconds(30)
        });
        if function.starts(&run_id) == 1 && waiting {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the failed first attempt was never put back behind its backoff \
             (starts {}, entry {:?})",
            function.starts(&run_id),
            entry.map(|e| (e.queue_status, e.available_at))
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    crud::request_cancel(&db, &run_id)
        .await
        .expect("request a stop");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let run = crud::get_run(&db, &run_id).await.unwrap().expect("run");
        if run.task_status.as_deref() == Some("cancelled") {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "a Stop during the backoff did not end the run; status {:?}",
            run.task_status
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(function.starts(&run_id), 1, "the waiting attempt never ran");
    let entry = crud::get_queue_entry(&db, &run_id)
        .await
        .unwrap()
        .expect("entry");
    assert_eq!(entry.queue_status, "cancelled");
}
