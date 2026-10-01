//! The `preview_compare` task on the worker fleet.

use std::sync::Arc;

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use futures::future::FutureExt;
use sea_orm::DatabaseConnection;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{PREVIEW_COMPARE_KIND, compare, enqueue};
use crate::agentic_wiring::preview_airhouse::{PreviewAirhousePorts, WorkspaceAirhouse};

/// Runs a `preview_compare` task. Registered in
/// `router::recovery::build_custom_task_registry`.
pub struct PreviewCompareExecutor {
    pub db: DatabaseConnection,
    pub airhouse: Arc<dyn PreviewAirhousePorts>,
}

impl PreviewCompareExecutor {
    /// On the workspace's own Airhouse.
    pub fn airhouse(db: DatabaseConnection) -> Self {
        Self {
            db,
            airhouse: WorkspaceAirhouse::shared(),
        }
    }
}

#[async_trait]
impl TaskExecutor for PreviewCompareExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let run_id = compare_run_id(&assignment.spec)?;
        let (event_tx, event_rx) = mpsc::channel(16);
        let (outcome_tx, outcome_rx) = mpsc::channel(4);
        let (db, airhouse) = (self.db.clone(), Arc::clone(&self.airhouse));
        tokio::spawn(async move {
            let started = serde_json::json!({ "preview_run_id": run_id });
            let _ = event_tx
                .send(("preview_compare_started".into(), started))
                .await;
            // A panic still owes the runtime a terminal outcome.
            let outcome = std::panic::AssertUnwindSafe(run_task(&db, airhouse.as_ref(), &run_id))
                .catch_unwind()
                .await
                .unwrap_or_else(|_| TaskOutcome::Failed("the preview compare panicked".into()));
            enqueue::mark_finished(&db, &run_id).await;
            let _ = outcome_tx.send(outcome).await;
        });
        Ok(ExecutingTask {
            events: event_rx,
            outcomes: outcome_rx,
            cancel: CancellationToken::new(),
            answers: None,
        })
    }
}

pub(super) fn compare_run_id(spec: &TaskSpec) -> Result<String, String> {
    match spec {
        TaskSpec::Custom { kind, payload } if kind == PREVIEW_COMPARE_KIND => payload
            .get("preview_run_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| "preview_compare payload missing string preview_run_id".into()),
        other => Err(format!(
            "unexpected spec for PreviewCompareExecutor: {other:?}"
        )),
    }
}

/// The stored outcome: the report, or a failure in fixed words
/// ([`super::CompareError`]).
async fn run_task(
    db: &DatabaseConnection,
    airhouse: &dyn PreviewAirhousePorts,
    run_id: &str,
) -> TaskOutcome {
    match compare(db, airhouse, run_id).await {
        Ok(report) => TaskOutcome::Done {
            answer: report.answer(),
            metadata: serde_json::to_value(&report).ok(),
        },
        Err(e) => {
            tracing::warn!(target: "preview", %run_id, error = %e, "preview compare failed");
            TaskOutcome::Failed(format!("preview compare failed: {e}"))
        }
    }
}
