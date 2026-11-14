//! A chunked airway backfill as a queued task:
//! `TaskSpec::Custom { kind: "airway_backfill_range", payload }`.
//!
//! `POST /agentic-airway/chunked-backfill` and `/resume-backfill` used to drive
//! the range in a detached task in the process that accepted the request. That
//! pinned both routes to the Factory (`OXY_ROLE=ide`): a `serve` replica runs
//! no workers, and a drive there would die with the pod. Now the request
//! records the range (or reads its coverage) and enqueues this task at
//! [`TaskScope::Global`](crud::TaskScope::Global); the worker fleet claims it,
//! rebuilds the workspace's `PlatformContext` from the run's `workspace_id`,
//! and drives the range there.
//!
//! **Re-running is safe.** A worker that dies mid-drive leaves its task to be
//! reclaimed, and the second drive runs the same range again: every chunk
//! checkpoint that reached `done` is skipped (`run_one_chunk`), so only the
//! unfinished periods run. That is also why the task carries no retry policy —
//! a chunk's failure is recorded on its checkpoint and a person resumes it,
//! exactly as before.
//!
//! **Rollout.** A worker one release behind has no handler for this kind and
//! fails the task with "no registered executor"; the range stays resumable, and
//! a Resume once the fleet has rolled drives it.

use std::sync::Arc;

use agentic_core::delegation::{TaskOutcome, TaskSpec};
use agentic_runtime::crud;
use agentic_runtime::worker::ExecutingTask;
use sea_orm::{DatabaseConnection, DbErr, TransactionTrait};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::backfill::{BackfillSummary, drive_backfill_range, resume_backfill_range};
use crate::platform::PlatformContext;

/// The `kind` of the task, and the `source_type` of its run.
pub const AIRWAY_BACKFILL_RANGE_KIND: &str = "airway_backfill_range";

/// What the run is called in the run lists.
const RUN_NAME: &str = "Airway chunked backfill";

/// One queued drive of a backfill range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackfillRangeTask {
    pub range_id: Uuid,
    /// `false`: drive the range's whole window (a new range). `true`: re-run
    /// only its not-`done` chunks (Resume).
    pub resume: bool,
}

/// Record a run for `task` in `workspace_id` and queue it for the worker fleet,
/// in one transaction. Returns the run id.
pub async fn enqueue_backfill_range(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    task: BackfillRangeTask,
) -> Result<String, DbErr> {
    let run_id = Uuid::new_v4().to_string();
    let spec = TaskSpec::Custom {
        kind: AIRWAY_BACKFILL_RANGE_KIND.to_string(),
        payload: serde_json::to_value(task).map_err(|e| DbErr::Custom(e.to_string()))?,
    };
    let metadata = serde_json::json!({
        "trigger": if task.resume { "resume" } else { "backfill" },
        "range_id": task.range_id,
    });
    let txn = db.begin().await?;
    crud::insert_run(
        &txn,
        &run_id,
        RUN_NAME,
        None,
        AIRWAY_BACKFILL_RANGE_KIND,
        Some(metadata),
        workspace_id,
    )
    .await?;
    crud::enqueue_task(
        &txn,
        &run_id,
        &run_id,
        None,
        &spec,
        None,
        crud::TaskScope::Global,
    )
    .await?;
    txn.commit().await?;
    Ok(run_id)
}

/// The worker side: drive the range on `platform` (the run's workspace) and
/// end the run with the tally, or with the reason the drive could not start.
pub(crate) fn execute_backfill_range(
    db: DatabaseConnection,
    platform: Arc<dyn PlatformContext>,
    payload: &serde_json::Value,
) -> Result<ExecutingTask, String> {
    let task: BackfillRangeTask = serde_json::from_value(payload.clone())
        .map_err(|e| format!("bad {AIRWAY_BACKFILL_RANGE_KIND} payload: {e}"))?;
    let (event_tx, events) = mpsc::channel(4);
    let (outcome_tx, outcomes) = mpsc::channel(1);
    agentic_runtime::hub_task::spawn_with_hub(async move {
        let _ = event_tx
            .send((
                "airway_backfill_range_started".into(),
                serde_json::json!({ "range_id": task.range_id, "resume": task.resume }),
            ))
            .await;
        let _ = outcome_tx.send(drive(&db, platform, task).await).await;
    });
    Ok(ExecutingTask {
        events,
        outcomes,
        cancel: CancellationToken::new(),
        answers: None,
    })
}

async fn drive(
    db: &DatabaseConnection,
    platform: Arc<dyn PlatformContext>,
    task: BackfillRangeTask,
) -> TaskOutcome {
    let result = if task.resume {
        resume_backfill_range(db, platform, task.range_id, None, |_| {}).await
    } else {
        drive_backfill_range(db, platform, task.range_id, None, |_| {}).await
    };
    match result {
        Ok(summary) => TaskOutcome::Done {
            answer: tally(&summary),
            metadata: Some(serde_json::json!({
                "range_id": task.range_id,
                "total": summary.total,
                "done": summary.done,
                "failed": summary.failed,
                "deferred": summary.deferred,
            })),
        },
        Err(e) => {
            tracing::error!(%e, range_id = %task.range_id, "airway backfill range drive failed");
            TaskOutcome::Failed(format!("backfill range {}: {e}", task.range_id))
        }
    }
}

/// The run's answer: what the drive did, in the coverage view's words.
fn tally(s: &BackfillSummary) -> String {
    format!(
        "{} of {} chunks done ({} resumed, {} degraded, {} failed, {} deferred)",
        s.done, s.total, s.resumed, s.degraded, s.failed, s.deferred
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_payload_round_trips_and_names_its_fields() {
        let task = BackfillRangeTask {
            range_id: Uuid::nil(),
            resume: true,
        };
        let value = serde_json::to_value(task).unwrap();
        assert_eq!(
            value,
            serde_json::json!({ "range_id": Uuid::nil(), "resume": true })
        );
        assert_eq!(
            serde_json::from_value::<BackfillRangeTask>(value).unwrap(),
            task
        );
    }

    #[test]
    fn a_payload_without_its_range_is_refused() {
        let decoded: Result<BackfillRangeTask, _> =
            serde_json::from_value(serde_json::json!({ "resume": false }));
        assert!(decoded.is_err());
    }

    #[test]
    fn the_tally_reads_like_the_coverage_view() {
        let s = BackfillSummary {
            total: 12,
            done: 10,
            resumed: 1,
            degraded: 0,
            failed: 1,
            deferred: 0,
        };
        assert_eq!(
            tally(&s),
            "10 of 12 chunks done (1 resumed, 0 degraded, 1 failed, 0 deferred)"
        );
    }
}
