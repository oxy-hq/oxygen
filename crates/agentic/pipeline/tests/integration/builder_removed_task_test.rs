//! A `__builder__` task still queued after the builder was removed is driven
//! by the real latency-worker path (`recover_pending_global_runs`: the lease,
//! the claim, `PipelineTaskExecutor`, the coordinator) and ends the run with
//! a terminal failure that names the removal.
//!
//! Run:
//!   cargo nextest run -p agentic-pipeline --test integration -E 'test(builder_removed_task_test)'

use std::sync::Arc;
use std::time::Duration;

use agentic_core::delegation::TaskSpec;
use agentic_pipeline::executor::BUILDER_AGENT_ID;
use agentic_pipeline::platform::IdentityResolver;
use agentic_pipeline::queued_run::{QueuedAnalyticsRun, seed_queued_analytics_run};
use agentic_pipeline::recovery::DrivePolicy;
use agentic_runtime::crud;
use agentic_runtime::state::RuntimeState;
use uuid::Uuid;

use crate::automation_recovery_test::{FakePlatform, test_db};

#[tokio::test(flavor = "multi_thread")]
async fn a_queued_builder_task_fails_the_run_with_a_terminal_builder_removed_error() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let run_id = Uuid::new_v4().to_string();
    let seeded = QueuedAnalyticsRun {
        run_id: run_id.clone(),
        workspace_id: ws,
        agent_id: BUILDER_AGENT_ID.into(),
        question: "add a view".into(),
        thread_id: None,
        thinking_mode: Default::default(),
        metadata: Default::default(),
        spec: TaskSpec::Agent {
            agent_id: BUILDER_AGENT_ID.into(),
            question: "add a view".into(),
            extra: None,
        },
    };
    seed_queued_analytics_run(&db, &seeded).await.expect("seed");

    agentic_pipeline::recovery::recover_pending_global_runs(
        db.clone(),
        Arc::new(RuntimeState::new()),
        Arc::new(FakePlatform),
        Arc::new(IdentityResolver),
        agentic_runtime::router::noop_router(),
        Some(ws),
        None,
        DrivePolicy::ALL,
    )
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let run = loop {
        let run = crud::get_run(&db, &run_id).await.unwrap().expect("run");
        if crud::user_facing_status(run.task_status.as_deref()) == "failed" {
            break run;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "run never failed: {:?}",
            run.task_status
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let message = run.error_message.unwrap_or_default();
    assert!(
        message.contains("builder was removed"),
        "run error: {message}"
    );

    let events = crud::get_all_events(&db, &run_id).await.unwrap();
    let failed = events
        .iter()
        .find(|e| e.event_type == "task_failed")
        .unwrap_or_else(|| {
            let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
            panic!("no task_failed event: {types:?}")
        });
    let error = failed.payload["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("builder was removed"),
        "event error: {error}"
    );
}
