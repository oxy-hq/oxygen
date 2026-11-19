//! The `custom_app_usage_purge` task: delete a deleted app's usage facts from
//! its workspace's Airhouse, on the worker fleet.
//!
//! Deleting an app removes its Postgres usage rows with the row itself (`ON
//! DELETE CASCADE`), but the facts in the workspace's Airhouse table
//! (`oxy_custom_app_usage`: viewer emails, event payloads) have no foreign key
//! to cascade through. The delete queues this task in the transaction that
//! removes the row, so an app is never gone without its purge queued, nor
//! purged without being gone.
//!
//! **Why a task and not an inline call** (`oxy-task-spec-default`): Airhouse is
//! a network hop that can be down at the moment of the delete, and the facts
//! must go anyway (the 30-day deletion commitment). The payload carries
//! everything the task needs, so it still runs after the `apps` row is gone.
//!
//! **The debt is a ledger row, not a queue row.** The delete writes a
//! `custom_app_usage_purges` row ([`ledger`]) beside the queued task. The queue
//! does not retry a failed custom task and prunes its failed rows after 30
//! days, so [`requeue_owed`] runs in the usage backfill's sweep
//! (`custom_apps_usage_backfill::spawn_sweep`, one claimed fleet job) and
//! queues a run for each owed purge that is due and has none waiting. A failed
//! run backs the next off; the ledger documents the schedule and when it gives up.

//! **Only ever the one app id the payload names.** There is deliberately no
//! sweep for "facts whose app is not in the apps table": a failed or empty
//! apps query would read as "every app was deleted".

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::orchestrator::crud::queue::TaskScope;
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::FutureExt;
use oxy_custom_apps::usage_airhouse::{self, SinkError};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub mod ledger;

/// The `TaskSpec::Custom` kind, and the run's `source_type` (a platform
/// daemon's: the run feed and workspace health leave it out).
pub const USAGE_PURGE_KIND: &str = "custom_app_usage_purge";

/// Apps queued again per sweep.
const REQUEUE_PER_SWEEP: i64 = 50;

/// The payload of one purge. Everything the task needs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsagePurgeTask {
    pub app_id: Uuid,
    /// The app's workspace: whose Airhouse holds the facts, and where the run
    /// is filed.
    pub workspace_id: Uuid,
}

impl UsagePurgeTask {
    /// The run (and task) id: an app is deleted once, so one per app.
    pub fn run_id(&self) -> String {
        format!("{USAGE_PURGE_KIND}:{}", self.app_id)
    }

    pub fn spec(&self) -> Result<TaskSpec, serde_json::Error> {
        Ok(TaskSpec::Custom {
            kind: USAGE_PURGE_KIND.to_string(),
            payload: serde_json::to_value(self)?,
        })
    }

    /// The payload of a queued spec.
    pub fn from_spec(spec: &TaskSpec) -> Result<Self, String> {
        match spec {
            TaskSpec::Custom { kind, payload } if kind == USAGE_PURGE_KIND => {
                serde_json::from_value(payload.clone())
                    .map_err(|e| format!("bad usage purge payload: {e}"))
            }
            other => Err(format!("not a usage purge task: {other:?}")),
        }
    }
}

/// Seed `task`'s run and queue it, on `conn`: the transaction that deletes the
/// app's row, so the two commit or roll back together.
pub async fn enqueue<C: ConnectionTrait>(conn: &C, task: &UsagePurgeTask) -> Result<(), DbErr> {
    enqueue_run(conn, task, task.run_id()).await
}

async fn enqueue_run<C: ConnectionTrait>(
    conn: &C,
    task: &UsagePurgeTask,
    run_id: String,
) -> Result<(), DbErr> {
    let spec = task.spec().map_err(|e| DbErr::Custom(e.to_string()))?;
    agentic_runtime::crud::insert_run(
        conn,
        &run_id,
        "Delete a deleted custom app's usage facts from its Airhouse",
        None,
        USAGE_PURGE_KIND,
        Some(json!({ "app_id": task.app_id, "workspace_id": task.workspace_id })),
        task.workspace_id,
    )
    .await?;
    agentic_runtime::crud::enqueue_task(
        conn,
        &run_id,
        &run_id,
        None,
        &spec,
        None,
        TaskScope::Global,
    )
    .await
}

/// Queue a new purge run for each owed purge due at `now` that has none
/// waiting or running. Returns how many it queued. Also prunes old completed
/// ledger rows.
pub async fn requeue_owed_at(db: &DatabaseConnection, now: DateTime<Utc>) -> Result<u64, DbErr> {
    if let Err(e) = ledger::prune_completed(db, now).await {
        tracing::warn!(error = %e, "{USAGE_PURGE_KIND}: pruning completed ledger rows failed");
    }
    let mut queued = 0;
    for task in ledger::due(db, now, REQUEUE_PER_SWEEP).await? {
        let run_id = format!("{}:{}", task.run_id(), now.timestamp_micros());
        enqueue_run(db, &task, run_id).await?;
        queued += 1;
    }
    Ok(queued)
}

/// [`requeue_owed_at`] now.
pub async fn requeue_owed(db: &DatabaseConnection) -> Result<u64, DbErr> {
    requeue_owed_at(db, Utc::now()).await
}

/// Delete the app's facts. `Ok(summary)`, or `Err(why not)`, and then the
/// queue retries. A workspace with no Airhouse has no facts: done.
pub async fn run(db: &DatabaseConnection, task: &UsagePurgeTask) -> Result<String, String> {
    match usage_airhouse::sink()
        .delete_app(db, task.workspace_id, task.app_id)
        .await
    {
        Ok(()) => Ok(format!("deleted the usage facts of app {}", task.app_id)),
        Err(SinkError::NotConfigured) => Ok(format!(
            "workspace {} has no Airhouse; no usage facts to delete",
            task.workspace_id
        )),
        Err(SinkError::Failed(why)) => Err(why),
    }
}

/// [`run`], then close or advance the task's ledger row as of `now`. The
/// ledger failing to record is logged and the run's own result returned: a row
/// left open is requeued by the sweep, which a deleted fact makes harmless.
pub async fn attempt(
    db: &DatabaseConnection,
    task: &UsagePurgeTask,
    now: DateTime<Utc>,
) -> Result<String, String> {
    let result = run(db, task).await;
    let app_id = task.app_id;
    match &result {
        Ok(_) => {
            if let Err(e) = ledger::complete(db, app_id, now).await {
                tracing::warn!(%app_id, error = %e, "{USAGE_PURGE_KIND}: ledger not closed");
            }
        }
        Err(why) => match ledger::fail(db, app_id, why, now).await {
            Ok(ledger::Failure::Abandoned { attempts }) => tracing::error!(
                %app_id, workspace_id = %task.workspace_id, attempts,
                "{USAGE_PURGE_KIND}: giving up; the app's usage facts are still in Airhouse \
                 (retry: clear abandoned_at and reset attempts on its custom_app_usage_purges row)"
            ),
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(%app_id, error = %e, "{USAGE_PURGE_KIND}: ledger not advanced")
            }
        },
    }
    result
}

/// Runs a queued [`UsagePurgeTask`] on the worker fleet.
pub struct UsagePurgeExecutor {
    pub db: DatabaseConnection,
}

#[async_trait]
impl TaskExecutor for UsagePurgeExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let task = UsagePurgeTask::from_spec(&assignment.spec)?;
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
async fn run_guarded(db: &DatabaseConnection, task: &UsagePurgeTask) -> TaskOutcome {
    let app_id = task.app_id;
    match std::panic::AssertUnwindSafe(attempt(db, task, Utc::now()))
        .catch_unwind()
        .await
    {
        Ok(Ok(summary)) => TaskOutcome::Done {
            answer: summary,
            metadata: Some(json!({ "app_id": app_id })),
        },
        Ok(Err(why)) => {
            tracing::warn!(%app_id, error = %why, "{USAGE_PURGE_KIND} failed; the ledger schedules the next run");
            TaskOutcome::Failed(format!("usage purge of app {app_id}: {why}"))
        }
        Err(_) => {
            tracing::error!(%app_id, "{USAGE_PURGE_KIND} panicked");
            TaskOutcome::Failed("the usage purge panicked".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_payload_round_trips_through_its_spec() {
        let task = UsagePurgeTask {
            app_id: Uuid::from_u128(1),
            workspace_id: Uuid::from_u128(2),
        };
        let spec = task.spec().unwrap();
        assert_eq!(UsagePurgeTask::from_spec(&spec).unwrap(), task);
    }

    #[test]
    fn a_foreign_spec_is_refused() {
        let spec = TaskSpec::Custom {
            kind: "something_else".into(),
            payload: json!({}),
        };
        assert!(UsagePurgeTask::from_spec(&spec).is_err());
    }

    #[test]
    fn a_run_id_names_its_app() {
        let task = UsagePurgeTask {
            app_id: Uuid::from_u128(7),
            workspace_id: Uuid::nil(),
        };
        assert!(task.run_id().ends_with(&Uuid::from_u128(7).to_string()));
    }
}
