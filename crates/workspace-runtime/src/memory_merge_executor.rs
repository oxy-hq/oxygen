//! `MemoryMergeExecutor`: runs `react_memory_merge` tasks
//! (`agentic_pipeline::queued_run::memory_merge`) on the worker fleet.
//!
//! One executor per process, holding only a `db` handle, like the preagg and
//! health-eval executors. Each task rebuilds the context its triggering run
//! was requested in ([`requester_platform`]), so the merge reads the agent and
//! its model key as that user, and the note owner is that user.

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_pipeline::queued_run::memory_merge::{
    MEMORY_MERGE_KIND, MemoryMergeRequest, MergeOutcome, run_memory_merge,
};
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::queued_analytics::requester::requester_platform;

/// How long a merge waits before its context is tried again (the workspace
/// is not compiled on this node yet, or a lookup failed), and for how long in
/// all before it is dead-lettered.
const RETRY_SECS: u64 = 60;
const MAX_WAIT_SECS: u64 = 30 * 60;

pub struct MemoryMergeExecutor {
    pub db: DatabaseConnection,
}

#[async_trait]
impl TaskExecutor for MemoryMergeExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let request = request_of(&assignment.spec)?;
        let (_event_tx, events) = mpsc::channel(1);
        let (outcome_tx, outcomes) = mpsc::channel(1);
        let db = self.db.clone();
        agentic_core::hub_task::spawn_with_hub(async move {
            let outcome = merge(&db, &request).await;
            if outcome_tx.send(outcome).await.is_err() {
                tracing::warn!("react memory merge: outcome dropped");
            }
        });
        Ok(ExecutingTask {
            events,
            outcomes,
            cancel: CancellationToken::new(),
            answers: None,
        })
    }
}

fn request_of(spec: &TaskSpec) -> Result<MemoryMergeRequest, String> {
    let TaskSpec::Custom { kind, payload } = spec else {
        return Err(format!("unexpected spec for MemoryMergeExecutor: {spec:?}"));
    };
    if kind != MEMORY_MERGE_KIND {
        return Err(format!("unknown memory merge kind: {kind}"));
    }
    serde_json::from_value(payload.clone()).map_err(|e| format!("bad {kind} payload: {e}"))
}

/// Runs the merge as the triggering run's requester. A run or requester that
/// is gone ends the task with nothing done: there is no one to merge as.
async fn merge(db: &DatabaseConnection, request: &MemoryMergeRequest) -> TaskOutcome {
    let done = |answer: serde_json::Value| TaskOutcome::Done {
        answer: answer.to_string(),
        metadata: None,
    };
    let skipped = |reason: &str| done(serde_json::json!({ "merged": false, "reason": reason }));
    let run = match agentic_runtime::crud::get_run(db, &request.source_run_id).await {
        Ok(Some(run)) => run,
        Ok(None) => return skipped("source run gone"),
        Err(e) => return retry(format!("source run lookup failed: {e}")),
    };
    let platform = match requester_platform(db, &run).await {
        Ok(Some(platform)) => platform,
        Ok(None) => return skipped("no requester"),
        Err(e) if e.is_transient() => return retry(e.to_string()),
        Err(e) => return TaskOutcome::Failed(format!("requester context: {e}")),
    };
    match run_memory_merge(&platform, db, request).await {
        Ok(outcome) => {
            if let MergeOutcome::Merged(r) = &outcome {
                tracing::info!(before = r.before, after = r.after, "react memory merged");
            }
            done(outcome.summary())
        }
        Err(e) => TaskOutcome::Failed(e),
    }
}

fn retry(reason: String) -> TaskOutcome {
    TaskOutcome::Deferred {
        delay_secs: RETRY_SECS,
        max_wait_secs: MAX_WAIT_SECS,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_memory_merge_spec_with_a_good_payload_is_taken() {
        let good = serde_json::json!({
            "workspace_id": uuid::Uuid::nil(), "user_id": uuid::Uuid::nil(),
            "agent_id": "agents/a.agentic.yml", "source_run_id": "r1",
        });
        let spec = |kind: &str, payload| TaskSpec::Custom {
            kind: kind.into(),
            payload,
        };
        assert_eq!(
            request_of(&spec(MEMORY_MERGE_KIND, good.clone()))
                .unwrap()
                .source_run_id,
            "r1"
        );
        assert!(request_of(&spec("preagg_cycle", good)).is_err());
        assert!(request_of(&spec(MEMORY_MERGE_KIND, serde_json::json!({}))).is_err());
    }
}
