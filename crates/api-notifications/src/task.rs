//! The delivery task's executor: the worker side of
//! `oxy_app_core::notifications::task`, which queues it. Delivery is the push,
//! email and Slack transport, so the executor is this crate's (handed to the
//! driver through `SurfaceSeams::tasks`).

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use futures::future::FutureExt;
use sea_orm::DatabaseConnection;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub use oxy_app_core::notifications::task::*;

pub struct NotificationDeliverExecutor {
    pub db: DatabaseConnection,
}

#[async_trait]
impl TaskExecutor for NotificationDeliverExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let TaskSpec::Custom { kind, payload } = &assignment.spec else {
            return Err(format!(
                "unexpected spec for NotificationDeliverExecutor: {:?}",
                assignment.spec
            ));
        };
        if kind != NOTIFICATION_DELIVER_KIND {
            return Err(format!("unknown notification-delivery kind: {kind}"));
        }
        let payload: DeliverPayload = serde_json::from_value(payload.clone())
            .map_err(|e| format!("bad notification-delivery payload: {e}"))?;

        let (event_tx, event_rx) = mpsc::channel(16);
        let (outcome_tx, outcome_rx) = mpsc::channel(4);
        let db = self.db.clone();
        tokio::spawn(async move {
            let _ = event_tx
                .send((
                    "notification_deliver_started".into(),
                    serde_json::json!({ "send_id": payload.send_id }),
                ))
                .await;
            let _ = outcome_tx
                .send(outcome_of(&db, payload.send_id).await)
                .await;
        });
        Ok(ExecutingTask {
            events: event_rx,
            outcomes: outcome_rx,
            cancel: CancellationToken::new(),
            answers: None,
        })
    }
}

/// Deliver and turn whatever happened into a terminal outcome — a panic
/// included, or the run would sit `running` with nothing left to end it.
async fn outcome_of(db: &DatabaseConnection, send_id: Uuid) -> TaskOutcome {
    let result = std::panic::AssertUnwindSafe(super::delivery::deliver_send(db, send_id))
        .catch_unwind()
        .await;
    match result {
        Ok(report) if report.failed == 0 => TaskOutcome::Done {
            answer: report.summary(),
            metadata: Some(serde_json::json!({ "send_id": send_id })),
        },
        // Failing the run is what schedules the retry, and the retry resends
        // only the `failed` rows.
        Ok(report) => TaskOutcome::Failed(report.summary()),
        Err(_) => {
            tracing::error!(%send_id, "notification delivery panicked");
            TaskOutcome::Failed("notification delivery panicked".to_string())
        }
    }
}
