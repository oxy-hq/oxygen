//! The `custom_app_usage_backfill` task: copy one workspace's custom-app usage
//! history from Oxy's Postgres into its Airhouse, on the worker fleet.
//!
//! The copy itself is `oxy_custom_apps::usage_backfill`; what is here is the
//! task around it — the kind, the payload, the queue — and the sweep that
//! queues one task for every workspace that has an Airhouse and is not yet
//! marked finished. Reads move to Airhouse for a workspace only after its pass
//! finishes (`oxy_custom_apps::usage_reads`).
//!
//! **One task per workspace, bounded.** A task copies for at most
//! [`PASS_BUDGET`] and then ends `Done`, saying it paused. The sweep, which
//! runs every [`SWEEP_INTERVAL`], queues the next one for a workspace that is
//! still unfinished and has none waiting or running; the saved position makes
//! the next task pick up where the last stopped. A task that fails (Airhouse
//! unreachable, a timeout) is its run's failure and a `warn!`; the sweep
//! queues it again.
//!
//! **Why a task and not an in-process spawn** (`oxy-task-spec-default`): it
//! takes minutes on a big history, touches the network, must survive a
//! restart, and can be retried.

use std::time::Duration;

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::orchestrator::crud::queue::TaskScope;
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use futures::FutureExt;
use oxy_custom_apps::usage_backfill::{self, Pass};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, SqlErr, Statement,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// The `TaskSpec::Custom` kind, and the run's `source_type` — a platform
/// daemon's, so the coordinator feed and workspace health both leave it out.
pub const USAGE_BACKFILL_KIND: &str = "custom_app_usage_backfill";

/// How long one task copies before it pauses.
pub const PASS_BUDGET: Duration = Duration::from_secs(10 * 60);
/// How often the sweep looks for workspaces that need a task.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// Most tasks one sweep queues, so a first sweep over many workspaces does not
/// put them all on the fleet at once.
pub const QUEUE_PER_SWEEP: u64 = 10;

/// The payload of one task: the workspace to copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageBackfillTask {
    pub workspace_id: Uuid,
}

impl UsageBackfillTask {
    pub fn spec(&self) -> Result<TaskSpec, serde_json::Error> {
        Ok(TaskSpec::Custom {
            kind: USAGE_BACKFILL_KIND.to_string(),
            payload: serde_json::to_value(self)?,
        })
    }

    /// The payload of a queued spec.
    pub fn from_spec(spec: &TaskSpec) -> Result<Self, String> {
        match spec {
            TaskSpec::Custom { kind, payload } if kind == USAGE_BACKFILL_KIND => {
                serde_json::from_value(payload.clone())
                    .map_err(|e| format!("bad usage backfill payload: {e}"))
            }
            other => Err(format!("not a usage backfill task: {other:?}")),
        }
    }
}

/// The run (and task) id of a task queued at `queued_at_micros`: the workspace
/// is in it, which is what [`is_pending`] looks for.
fn run_id_of(workspace_id: Uuid, queued_at_micros: i64) -> String {
    format!("{USAGE_BACKFILL_KIND}:{workspace_id}:{queued_at_micros}")
}

/// Whether the queue holds a task for `workspace_id` that is waiting or running.
pub async fn is_pending(db: &DatabaseConnection, workspace_id: Uuid) -> Result<bool, DbErr> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT 1 AS pending FROM agentic_task_queue \
              WHERE task_id LIKE $1 AND queue_status IN ('queued', 'claimed') LIMIT 1",
            [format!("{USAGE_BACKFILL_KIND}:{workspace_id}:%").into()],
        ))
        .await?;
    Ok(row.is_some())
}

/// Queue a task for `workspace_id`. `Ok(false)` when one is already waiting or
/// running, or this exact run exists.
pub async fn enqueue(db: &DatabaseConnection, workspace_id: Uuid) -> Result<bool, DbErr> {
    if is_pending(db, workspace_id).await? {
        return Ok(false);
    }
    // Counts as an attempt, so a workspace whose passes keep failing sorts
    // behind the ones not yet tried (`unfinished_workspaces`).
    usage_backfill::note_attempt(db, workspace_id).await?;
    let task = UsageBackfillTask { workspace_id };
    let run_id = run_id_of(workspace_id, chrono::Utc::now().timestamp_micros());
    let spec = task.spec().map_err(|e| DbErr::Custom(e.to_string()))?;
    let txn = db.begin().await?;
    let inserted = agentic_runtime::crud::insert_run(
        &txn,
        &run_id,
        "Copy a workspace's custom-app usage history to its Airhouse",
        None,
        USAGE_BACKFILL_KIND,
        Some(json!({ "workspace_id": workspace_id })),
        workspace_id,
    )
    .await;
    match inserted {
        Ok(()) => {}
        Err(e) if matches!(e.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) => {
            txn.rollback().await?;
            return Ok(false);
        }
        Err(e) => return Err(e),
    }
    agentic_runtime::crud::enqueue_task(
        &txn,
        &run_id,
        &run_id,
        None,
        &spec,
        None,
        TaskScope::Global,
    )
    .await?;
    txn.commit().await?;
    tracing::info!(%workspace_id, %run_id, "custom-app usage backfill queued");
    Ok(true)
}

/// Queue a task for each workspace that needs one, at most
/// [`QUEUE_PER_SWEEP`]: a finished workspace with rows parked in Postgres
/// first, then one not yet finished. Returns how many it queued.
pub async fn queue_unfinished(db: &DatabaseConnection) -> Result<u64, DbErr> {
    let mut queued = 0;
    // A finished workspace with rows parked in Postgres after a failed append
    // first: those rows are missing from what its reads see until copied.
    let parked = usage_backfill::parked_workspaces(db, QUEUE_PER_SWEEP).await?;
    let unfinished = usage_backfill::unfinished_workspaces(db, QUEUE_PER_SWEEP * 4).await?;
    for workspace_id in parked.into_iter().chain(unfinished) {
        if queued >= QUEUE_PER_SWEEP {
            break;
        }
        if enqueue(db, workspace_id).await? {
            queued += 1;
        }
    }
    Ok(queued)
}

/// Run [`queue_unfinished`] every [`SWEEP_INTERVAL`] until shutdown. Singleton:
/// two replicas sweeping would only race to queue the same tasks.
pub fn spawn_sweep(db: DatabaseConnection, shutdown: CancellationToken, is_singleton_role: bool) {
    if !is_singleton_role {
        return;
    }
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
        // Startup is busy enough; the first real tick is minutes away.
        ticker.tick().await;
        loop {
            tokio::select! {
                _ = ticker.tick() => match queue_unfinished(&db).await {
                    Ok(0) => {}
                    Ok(queued) => tracing::info!(queued, "custom-app usage backfill swept"),
                    Err(e) => tracing::warn!(error = %e, "custom-app usage backfill sweep failed"),
                },
                _ = shutdown.cancelled() => return,
            }
        }
    });
}

/// Runs a queued [`UsageBackfillTask`] on the worker fleet.
pub struct UsageBackfillExecutor {
    pub db: DatabaseConnection,
}

#[async_trait]
impl TaskExecutor for UsageBackfillExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let task = UsageBackfillTask::from_spec(&assignment.spec)?;
        let (event_tx, event_rx) = mpsc::channel(1);
        let (outcome_tx, outcome_rx) = mpsc::channel(1);
        let cancel = CancellationToken::new();
        let db = self.db.clone();
        let stop = cancel.clone();
        tokio::spawn(async move {
            // A cancelled task leaves its position saved; the next pass
            // resumes from it.
            let outcome = tokio::select! {
                outcome = run_guarded(&db, task) => outcome,
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
async fn run_guarded(db: &DatabaseConnection, task: UsageBackfillTask) -> TaskOutcome {
    let workspace_id = task.workspace_id;
    let run = std::panic::AssertUnwindSafe(usage_backfill::run(db, workspace_id, PASS_BUDGET))
        .catch_unwind()
        .await;
    let metadata = json!({ "workspace_id": workspace_id });
    match run {
        Ok(Ok(pass)) => TaskOutcome::Done {
            answer: describe(pass),
            metadata: Some(metadata),
        },
        Ok(Err(e)) => {
            tracing::warn!(%workspace_id, error = %e, "{USAGE_BACKFILL_KIND} failed");
            TaskOutcome::Failed(format!("usage backfill of workspace {workspace_id}: {e}"))
        }
        Err(_) => {
            tracing::error!(%workspace_id, "{USAGE_BACKFILL_KIND} panicked");
            TaskOutcome::Failed("the usage backfill panicked".to_string())
        }
    }
}

fn describe(pass: Pass) -> String {
    match pass {
        Pass::NoAirhouse => "the workspace has no Airhouse; nothing was copied".to_string(),
        Pass::AlreadyFinished => "already finished; nothing was copied".to_string(),
        Pass::Finished { rows } => {
            format!("finished: {rows} rows handed to Airhouse in this pass")
        }
        Pass::Paused { rows } => {
            format!("paused after {rows} rows; the next pass resumes from the saved position")
        }
        Pass::CopiedParked {
            rows,
            cleared: true,
        } => format!("copied {rows} parked rows to Airhouse"),
        Pass::ParkedPaused { rows, moved: true } => format!(
            "paused after copying {rows} parked rows; the next pass starts where this one got"
        ),
        Pass::ParkedPaused { rows, moved: false } => format!(
            "paused after copying {rows} parked rows; more were parked meanwhile, so the next \
             pass starts at the oldest of them"
        ),
        Pass::CopiedParked {
            rows,
            cleared: false,
        } => format!(
            "copied {rows} parked rows to Airhouse; more were parked meanwhile, so the next \
             sweep queues another pass"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_payload_round_trips_through_its_spec() {
        let task = UsageBackfillTask {
            workspace_id: Uuid::from_u128(7),
        };
        let spec = task.spec().unwrap();
        assert_eq!(UsageBackfillTask::from_spec(&spec).unwrap(), task);
        assert!(
            UsageBackfillTask::from_spec(&TaskSpec::Custom {
                kind: "other".into(),
                payload: json!({}),
            })
            .is_err()
        );
    }

    #[test]
    fn a_run_id_names_its_workspace() {
        let ws = Uuid::from_u128(9);
        assert!(run_id_of(ws, 5).starts_with(&format!("{USAGE_BACKFILL_KIND}:{ws}:")));
    }
}
