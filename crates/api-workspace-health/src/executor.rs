//! `HealthEvalTaskExecutor`: runs a per-workspace `health_eval_workspace`
//! `TaskSpec::Custom` on the global-run fleet. Registered into the
//! `CustomTaskRegistry` by the in-process driver
//! (`server::router::recovery`); `PipelineTaskExecutor` delegates the
//! `health_eval_workspace` kind here. The actual evaluation is the unchanged
//! [`run_eval_pass_single`]. The registry mechanism is
//! `internal-docs/agentic-runtime-integration.md` ("One-shot queue work:
//! `TaskSpec::Custom` + `CustomTaskRegistry`"); what this eval means for a
//! tenant is `internal-docs/admin-surfaces.md` ("Workspace health → who
//! actually gets evaluated").

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use futures::future::FutureExt;
use sea_orm::DatabaseConnection;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::run_eval_pass_single;

/// The `kind` discriminator for the per-workspace health eval Custom task.
pub const HEALTH_EVAL_KIND: &str = "health_eval_workspace";

pub struct HealthEvalTaskExecutor {
    pub db: DatabaseConnection,
}

#[async_trait]
impl TaskExecutor for HealthEvalTaskExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let TaskSpec::Custom { kind, payload } = &assignment.spec else {
            return Err(format!(
                "unexpected spec for HealthEvalTaskExecutor: {:?}",
                assignment.spec
            ));
        };
        if kind != HEALTH_EVAL_KIND {
            return Err(format!("unknown health kind: {kind}"));
        }
        let workspace_id: uuid::Uuid = payload
            .get("workspace_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "health_eval payload missing string workspace_id".to_string())?
            .parse()
            .map_err(|e| format!("bad workspace_id: {e}"))?;
        // Absent → false: a scheduled fire, and any task enqueued by an older
        // instance mid-deploy, keeps the workspace's own smoke cadence. Only the
        // operator-triggered "Run smoke test" sets it.
        let force_smoke = payload
            .get("force_smoke")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let (event_tx, event_rx) = mpsc::channel(16);
        let (outcome_tx, outcome_rx) = mpsc::channel(4);
        let cancel = CancellationToken::new();
        let db = self.db.clone();

        tokio::spawn(async move {
            run_health_eval_task(db, workspace_id, force_smoke, event_tx, outcome_tx).await;
        });

        Ok(ExecutingTask {
            events: event_rx,
            outcomes: outcome_rx,
            cancel,
            answers: None,
        })
    }
}

async fn run_health_eval_task(
    db: DatabaseConnection,
    workspace_id: uuid::Uuid,
    force_smoke: bool,
    event_tx: mpsc::Sender<(String, serde_json::Value)>,
    outcome_tx: mpsc::Sender<TaskOutcome>,
) {
    let _ = event_tx
        .send((
            "health_eval_started".into(),
            serde_json::json!({ "workspace_id": workspace_id, "force_smoke": force_smoke }),
        ))
        .await;
    // Guard against a panic inside the eval: if the future unwinds we still owe
    // the runtime a terminal outcome, otherwise dropping `outcome_tx` with
    // nothing sent leaves the run stuck `running` forever with no terminal event.
    let result = std::panic::AssertUnwindSafe(run_eval_pass_single(&db, workspace_id, force_smoke))
        .catch_unwind()
        .await;
    let outcome = match result {
        Ok(Ok(summary)) => TaskOutcome::Done {
            answer: summary,
            metadata: Some(serde_json::json!({ "workspace_id": workspace_id })),
        },
        Ok(Err(e)) => TaskOutcome::Failed(e),
        Err(panic) => {
            let msg = panic_message(&panic);
            tracing::error!(target: "health_eval", %workspace_id, panic = %msg,
                "health eval task panicked");
            TaskOutcome::Failed(format!("health eval panicked: {msg}"))
        }
    };
    let _ = outcome_tx.send(outcome).await;
}

/// Best-effort string from a caught panic payload (the usual `&str` / `String`).
fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{SKIP_MSG, test_db};

    // Constructing the executor needs a connection even though the negative
    // paths reject before querying.

    fn assignment(spec: TaskSpec) -> TaskAssignment {
        TaskAssignment {
            task_id: "t".into(),
            parent_task_id: None,
            run_id: "r".into(),
            spec,
            policy: None,
            claim_epoch: agentic_core::NO_CLAIM,
        }
    }

    #[tokio::test]
    async fn rejects_wrong_kind() {
        let Some(db) = test_db().await else {
            eprintln!("{SKIP_MSG}");
            return;
        };
        let exec = HealthEvalTaskExecutor { db };
        let err = match exec
            .execute(assignment(TaskSpec::Custom {
                kind: "preagg_cycle".into(),
                payload: serde_json::json!({}),
            }))
            .await
        {
            Ok(_) => panic!("expected wrong-kind rejection"),
            Err(e) => e,
        };
        assert!(err.contains("preagg_cycle"), "got: {err}");
    }

    #[tokio::test]
    async fn rejects_missing_workspace_id() {
        let Some(db) = test_db().await else {
            eprintln!("{SKIP_MSG}");
            return;
        };
        let exec = HealthEvalTaskExecutor { db };
        let err = match exec
            .execute(assignment(TaskSpec::Custom {
                kind: HEALTH_EVAL_KIND.into(),
                payload: serde_json::json!({}),
            }))
            .await
        {
            Ok(_) => panic!("expected missing-workspace_id rejection"),
            Err(e) => e,
        };
        assert!(err.to_lowercase().contains("workspace_id"), "got: {err}");
    }

    #[tokio::test]
    async fn rejects_non_custom_spec() {
        let Some(db) = test_db().await else {
            eprintln!("{SKIP_MSG}");
            return;
        };
        let exec = HealthEvalTaskExecutor { db };
        let err = match exec
            .execute(assignment(TaskSpec::Agent {
                agent_id: "a".into(),
                question: "q".into(),
                extra: None,
            }))
            .await
        {
            Ok(_) => panic!("expected non-Custom rejection"),
            Err(e) => e,
        };
        assert!(err.contains("unexpected spec"), "got: {err}");
    }
}
