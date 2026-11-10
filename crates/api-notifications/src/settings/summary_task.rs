//! The quiet-hours summary as a durable task: queued not to run before the
//! window ends (`enqueue_task_at`), drained by whichever pod takes it. What
//! it does is [`super::summary::summarize`]; this is the queue plumbing,
//! after `notifications::task`.
//!
//! One task per (person, window end): its id names both, and queueing takes a
//! transaction-scoped advisory lock on that id and checks for the run first,
//! so two delivery runs holding pushes for one person in one window queue it
//! once.
//!
//! Rollout: the kind and its enqueue ship together, and the first one is due
//! hours after it is queued, long after the deploy has finished. On a
//! re-pinned previous release the task fails as an unknown kind; the holds
//! stay in the inbox.

use agentic_core::delegation::{
    BackoffStrategy, RetryPolicy, TaskAssignment, TaskOutcome, TaskPolicy, TaskSpec,
};
use agentic_runtime::crud::queue::enqueue_task_at;
use agentic_runtime::crud::{TaskScope, insert_run};
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::summary::{Summary, summarize};

/// The `kind` of the summary task, and the `source_type` of its run.
pub const QUIET_SUMMARY_KIND: &str = "notification_quiet_summary";

/// What the run is called in the Orchestrator Dashboard.
const RUN_NAME: &str = "Quiet hours summary";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryPayload {
    pub user_id: Uuid,
    /// Where the run is recorded, and where a rescheduled one goes.
    pub workspace_id: Uuid,
}

/// The run and task id for `user_id`'s summary of the window ending `due`.
pub fn run_id(user_id: Uuid, due: DateTime<Utc>) -> String {
    format!("notification-quiet-summary-{user_id}-{}", due.timestamp())
}

/// A failed read is worth a retry; a failed push is not (it may have gone).
fn policy() -> TaskPolicy {
    TaskPolicy {
        retry: Some(RetryPolicy {
            max_retries: 2,
            backoff: BackoffStrategy::Exponential {
                initial_delay_ms: 30_000,
                max_delay_ms: 300_000,
            },
            retry_on: Vec::new(),
        }),
        fallback_targets: Vec::new(),
    }
}

/// Queue `user_id`'s summary, due at `due`, unless it is already queued.
/// `true` when this call queued it.
pub async fn schedule(
    db: &DatabaseConnection,
    user_id: Uuid,
    due: DateTime<Utc>,
    workspace_id: Uuid,
) -> Result<bool, DbErr> {
    let run_id = run_id(user_id, due);
    let txn = db.begin().await?;
    txn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
        [run_id.clone().into()],
    ))
    .await?;
    let queued = txn
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT 1 AS one FROM agentic_runs WHERE id = $1",
            [run_id.clone().into()],
        ))
        .await?
        .is_some();
    if queued {
        txn.rollback().await?;
        return Ok(false);
    }
    let payload = SummaryPayload {
        user_id,
        workspace_id,
    };
    let spec = TaskSpec::Custom {
        kind: QUIET_SUMMARY_KIND.to_string(),
        payload: serde_json::to_value(&payload).map_err(|e| DbErr::Custom(e.to_string()))?,
    };
    let metadata = serde_json::json!({ "trigger": "quiet_hours", "due": due.to_rfc3339() });
    insert_run(
        &txn,
        &run_id,
        RUN_NAME,
        None,
        QUIET_SUMMARY_KIND,
        Some(metadata),
        workspace_id,
    )
    .await?;
    let not_before = due.fixed_offset();
    enqueue_task_at(
        &txn,
        &run_id,
        &run_id,
        None,
        &spec,
        Some(&policy()),
        TaskScope::Global,
        not_before,
    )
    .await?;
    txn.commit().await?;
    Ok(true)
}

pub struct QuietSummaryExecutor {
    pub db: DatabaseConnection,
}

#[async_trait]
impl TaskExecutor for QuietSummaryExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let TaskSpec::Custom { kind, payload } = &assignment.spec else {
            return Err(format!(
                "unexpected spec for QuietSummaryExecutor: {:?}",
                assignment.spec
            ));
        };
        if kind != QUIET_SUMMARY_KIND {
            return Err(format!("unknown quiet-summary kind: {kind}"));
        }
        let payload: SummaryPayload = serde_json::from_value(payload.clone())
            .map_err(|e| format!("bad quiet-summary payload: {e}"))?;
        let (event_tx, event_rx) = mpsc::channel(4);
        let (outcome_tx, outcome_rx) = mpsc::channel(1);
        let db = self.db.clone();
        tokio::spawn(async move {
            let _ = event_tx
                .send((
                    "notification_quiet_summary_started".into(),
                    serde_json::json!({ "user_id": payload.user_id }),
                ))
                .await;
            let _ = outcome_tx.send(outcome_of(&db, &payload).await).await;
        });
        Ok(ExecutingTask {
            events: event_rx,
            outcomes: outcome_rx,
            cancel: CancellationToken::new(),
            answers: None,
        })
    }
}

/// When a summary that found the person still quiet runs next: the window's
/// end, or — should that not be after `now` (a clock at an edge) — a minute
/// from now. Never `now` or earlier: that is this run's own id, which
/// [`schedule`] finds already queued and so queues nothing, and the holds
/// would wait for a window a day later and fall out of the summary's reach.
pub fn moved_to(end: DateTime<Utc>, now: DateTime<Utc>) -> DateTime<Utc> {
    match end > now {
        true => end,
        false => now + chrono::Duration::minutes(1),
    }
}

/// Summarize and turn whatever happened into a terminal outcome.
async fn outcome_of(db: &DatabaseConnection, payload: &SummaryPayload) -> TaskOutcome {
    let user = payload.user_id;
    let now = Utc::now();
    match summarize(db, user, now).await {
        Ok(Summary::StillQuiet(end)) => {
            let due = moved_to(end, now);
            let answer = match schedule(db, user, due, payload.workspace_id).await {
                Ok(_) => format!("still quiet; summary moved to {}", due.to_rfc3339()),
                Err(e) => {
                    tracing::warn!(%user, error = %e, "quiet-hours summary not rescheduled");
                    "still quiet; summary could not be moved".to_string()
                }
            };
            TaskOutcome::Done {
                answer,
                metadata: None,
            }
        }
        Ok(summary) => TaskOutcome::Done {
            answer: format!("{summary:?}"),
            metadata: None,
        },
        Err(e) => TaskOutcome::Failed(format!("quiet-hours summary could not run: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_summary_still_quiet_moves_strictly_later_under_a_new_id() {
        let at = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc);
        let now = at("2026-10-25T01:15:00Z");
        let ahead = at("2026-10-25T01:30:00Z");
        assert_eq!(moved_to(ahead, now), ahead);
        let user = Uuid::new_v4();
        for past in [now, at("2026-10-25T00:30:00Z")] {
            let due = moved_to(past, now);
            assert!(due > now, "{past}");
            assert_ne!(
                run_id(user, due),
                run_id(user, past),
                "a fresh run, not this one"
            );
        }
    }
}
