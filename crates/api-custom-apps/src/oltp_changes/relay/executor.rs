//! The relay as a queued task, run on the worker fleet the way the app events
//! relay is (`app_events::relay`): [`enqueue_relay`] queues one pass for an
//! (org, environment) unless one is already waiting, and [`RelayExecutor`]
//! runs it, a panic included in its outcome.

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::crud::{TaskScope, enqueue_task, insert_run};
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use chrono::Utc;
use futures::FutureExt;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{RELAY_KIND, relay_once};
use crate::app_events::relay::run_workspace;

/// The payload of one relay task: whose change stream, in which environment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct RelayPayload {
    org_id: Uuid,
    environment: String,
}

impl RelayPayload {
    fn spec(&self) -> Result<TaskSpec, DbErr> {
        Ok(TaskSpec::Custom {
            kind: RELAY_KIND.to_string(),
            payload: serde_json::to_value(self).map_err(|e| DbErr::Custom(e.to_string()))?,
        })
    }

    fn from_spec(spec: &TaskSpec) -> Result<Self, String> {
        match spec {
            TaskSpec::Custom { kind, payload } if kind == RELAY_KIND => {
                serde_json::from_value(payload.clone())
                    .map_err(|e| format!("bad OLTP change stream relay payload: {e}"))
            }
            other => Err(format!("not an OLTP change stream relay task: {other:?}")),
        }
    }
}

/// Queue a relay pass of `org_id`'s `environment` change stream, unless one
/// is already waiting to run (one that is running may have read its batches
/// before the change that asked for this one committed, so it does not
/// count). `false` when the org has no app to queue it under.
pub async fn enqueue_relay(
    db: &DatabaseConnection,
    org_id: Uuid,
    environment: &str,
) -> Result<bool, DbErr> {
    let prefix = format!("{RELAY_KIND}:{org_id}:{environment}:");
    let waiting = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT 1 AS waiting FROM agentic_task_queue \
              WHERE task_id LIKE $1 AND queue_status = 'queued' LIMIT 1",
            [format!("{prefix}%").into()],
        ))
        .await?;
    if waiting.is_some() {
        return Ok(true);
    }
    let Some(workspace_id) = run_workspace(db, org_id).await? else {
        tracing::warn!(%org_id, environment, "OLTP change stream relay not queued: the org has no app");
        return Ok(false);
    };
    let payload = RelayPayload {
        org_id,
        environment: environment.to_string(),
    };
    let spec = payload.spec()?;
    let run_id = format!("{prefix}{}", Utc::now().timestamp_micros());
    let txn = db.begin().await?;
    insert_run(
        &txn,
        &run_id,
        "Relay the OLTP change stream",
        None,
        RELAY_KIND,
        Some(json!({ "org_id": org_id, "environment": environment })),
        workspace_id,
    )
    .await?;
    enqueue_task(&txn, &run_id, &run_id, None, &spec, None, TaskScope::Global).await?;
    txn.commit().await?;
    Ok(true)
}

/// Runs a queued relay pass on the worker fleet.
pub struct RelayExecutor {
    pub db: DatabaseConnection,
}

#[async_trait]
impl TaskExecutor for RelayExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let payload = RelayPayload::from_spec(&assignment.spec)?;
        let (event_tx, event_rx) = mpsc::channel(1);
        let (outcome_tx, outcome_rx) = mpsc::channel(1);
        let cancel = CancellationToken::new();
        let db = self.db.clone();
        let stop = cancel.clone();
        tokio::spawn(async move {
            // A pass dropped mid-way leaves its batch declared and unconfirmed,
            // and its lease held until it expires: the next pass's check sees
            // a crash, and ships the batch again.
            let outcome = tokio::select! {
                outcome = run_guarded(&db, &payload) => outcome,
                _ = stop.cancelled() => TaskOutcome::Failed("cancelled".to_string()),
            };
            let _ = outcome_tx.send(outcome).await;
            drop(event_tx);
        });
        Ok(ExecutingTask {
            events: event_rx,
            outcomes: outcome_rx,
            cancel,
            answers: None,
        })
    }
}

/// The task's outcome. A panic still ends in `Failed`, or the run would stay
/// `running` with no terminal event.
async fn run_guarded(db: &DatabaseConnection, payload: &RelayPayload) -> TaskOutcome {
    let RelayPayload {
        org_id,
        environment,
    } = payload;
    let pass = std::panic::AssertUnwindSafe(relay_once(db, *org_id, environment))
        .catch_unwind()
        .await;
    match pass {
        Ok(Ok(stats)) => TaskOutcome::Done {
            answer: if stats.skipped_lease {
                "another relay holds the lease; nothing relayed".to_string()
            } else {
                format!(
                    "shipped {} changes in {} batches; {} held",
                    stats.shipped, stats.batches, stats.held
                )
            },
            metadata: Some(json!({ "org_id": org_id, "environment": environment })),
        },
        Ok(Err(e)) => {
            tracing::warn!(%org_id, environment, error = %e, "{RELAY_KIND} failed");
            TaskOutcome::Failed(e)
        }
        Err(_) => {
            tracing::error!(%org_id, environment, "{RELAY_KIND} panicked");
            TaskOutcome::Failed("the OLTP change stream relay panicked".to_string())
        }
    }
}

#[cfg(test)]
mod tests;
