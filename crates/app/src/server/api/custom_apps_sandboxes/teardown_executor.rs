//! Runs a queued sandbox teardown ([`super::teardown`]) on the worker fleet.
//! Registered for `SANDBOX_TEARDOWN_KIND` by
//! `server::router::recovery::build_custom_task_registry`.
//!
//! The outcome is the record: `Done` with what was removed, or `Failed` with
//! why not — and then the sandbox's row is still there, still `deleting`.

use agentic_core::delegation::{TaskAssignment, TaskOutcome};
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use futures::FutureExt;
use sea_orm::DatabaseConnection;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::teardown::{SANDBOX_TEARDOWN_KIND, SandboxTeardownTask, run};

pub struct SandboxTeardownExecutor {
    pub db: DatabaseConnection,
}

#[async_trait]
impl TaskExecutor for SandboxTeardownExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let task = SandboxTeardownTask::from_spec(&assignment.spec)?;
        let (event_tx, event_rx) = mpsc::channel(1);
        let (outcome_tx, outcome_rx) = mpsc::channel(1);
        let db = self.db.clone();
        tokio::spawn(async move {
            let outcome = run_guarded(&db, &task).await;
            let _ = outcome_tx.send(outcome).await;
            drop(event_tx);
        });
        Ok(ExecutingTask {
            events: event_rx,
            outcomes: outcome_rx,
            cancel: CancellationToken::new(),
            answers: None,
        })
    }
}

/// The task's outcome. A panic still ends in `Failed`, or the run would stay
/// `running` with no terminal event.
async fn run_guarded(db: &DatabaseConnection, task: &SandboxTeardownTask) -> TaskOutcome {
    let run = std::panic::AssertUnwindSafe(run(db, task))
        .catch_unwind()
        .await;
    let metadata = json!({
        "app_id": task.app_id,
        "environment": task.environment,
        "reason": task.reason,
    });
    match run {
        Ok(Ok(summary)) => TaskOutcome::Done {
            answer: summary,
            metadata: Some(metadata),
        },
        Ok(Err(why)) => {
            tracing::warn!(app_id = %task.app_id, environment = %task.environment,
                "{SANDBOX_TEARDOWN_KIND}: {why}");
            TaskOutcome::Failed(why)
        }
        Err(_) => {
            tracing::error!(app_id = %task.app_id, environment = %task.environment,
                "{SANDBOX_TEARDOWN_KIND} panicked");
            TaskOutcome::Failed("the sandbox teardown panicked".to_string())
        }
    }
}
