//! `TaskSpec::Custom { kind: "experiment_check", payload: { run_id } }`,
//! drained by the global-run fleet like the health eval. The workspace context
//! is `build_workspace_ctx`: the promoted revision's semantic model, never a
//! working copy (a fleet node may not have one, and an IDE node's holds
//! uncommitted edits).

use std::sync::Arc;

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_pipeline::platform::ProjectContext;
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use oxy_experiment::run::RunState;
use sea_orm::{DatabaseConnection, EntityTrait};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::reader::{Axis, MonitorAnomalyScan, PanelGroups, RunnerUnitDays};
use super::{Readers, run_check};
use crate::server::experiment::jobs::CHECK_KIND;
use crate::server::experiment::panel::PanelContext;
use crate::server::experiment::store;
use crate::server::router::recovery::build_workspace_ctx;

pub struct ExperimentCheckExecutor {
    pub db: DatabaseConnection,
}

pub fn payload_run_id(spec: &TaskSpec) -> Result<Uuid, String> {
    let TaskSpec::Custom { kind, payload } = spec else {
        return Err(format!(
            "unexpected spec for ExperimentCheckExecutor: {spec:?}"
        ));
    };
    if kind != CHECK_KIND {
        return Err(format!("ExperimentCheckExecutor does not run `{kind}`"));
    }
    payload
        .get("run_id")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| "experiment_check payload has no run_id".to_string())
}

#[async_trait]
impl TaskExecutor for ExperimentCheckExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let run_id = payload_run_id(&assignment.spec)?;
        let (event_tx, event_rx) = mpsc::channel(8);
        let (outcome_tx, outcome_rx) = mpsc::channel(1);
        let db = self.db.clone();
        tokio::spawn(async move {
            let _events = event_tx; // no progress events; held open until the outcome
            let outcome = match check_and_record(&db, run_id, Utc::now()).await {
                Ok(answer) => TaskOutcome::Done {
                    answer,
                    metadata: None,
                },
                Err(e) => TaskOutcome::Failed(e),
            };
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

/// The check, and — when it cannot run — the reason, written on the run so its
/// status shows it. The job's own `Failed` outcome is invisible there.
pub async fn check_and_record(
    db: &DatabaseConnection,
    run_id: Uuid,
    now: DateTime<Utc>,
) -> Result<String, String> {
    let result = check_workspace(db, run_id, now).await;
    if let Err(reason) = &result {
        if let Err(e) = store::record_check_failure(db, run_id, reason, now).await {
            tracing::warn!(
                target: "experiment", %run_id, error = %e,
                "a failed check could not be recorded on the run"
            );
        }
    }
    result
}

async fn check_workspace(
    db: &DatabaseConnection,
    run_id: Uuid,
    now: DateTime<Utc>,
) -> Result<String, String> {
    let run = store::run_by_id(db, run_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("experiment run {run_id} not found"))?;
    if RunState::parse(&run.state).is_some_and(RunState::is_terminal) {
        return Ok(format!("run {} is {}; nothing to check", run.id, run.state));
    }
    let c = store::pinned_config(db, &run).await?;
    let ctx = build_workspace_ctx(run.workspace_id, db)
        .await
        .ok_or("the workspace context could not be built on this node")?;
    let runner = ctx
        .metric_tree_runner_system()
        .ok_or("the workspace has no metric-tree runner")?;
    let layer = runner
        .load_layer()
        .await
        .map_err(|e| format!("loading the semantic model: {e:?}"))?;
    let days = RunnerUnitDays {
        runner: Arc::clone(&runner),
        layer: layer.clone(),
        axis: Axis::of(&c),
        tz: c.design.timezone,
    };
    let anomalies = MonitorAnomalyScan {
        runner,
        layer,
        axis: Axis::of(&c),
        tz: c.design.timezone,
    };
    let groups = PanelGroups {
        db: db.clone(),
        ctx: panel_context(db, run.workspace_id).await?,
    };
    let readers = Readers {
        days: &days,
        anomalies: &anomalies,
        groups: &groups,
    };
    run_check(db, &run, &readers, now).await
}

/// PR 3's panel context for the promoted revision. A queued job has no
/// caller, so it reads as the nil user (the index's decision 3).
/// Shared with the estimate job (`estimate::engine_stage`).
pub(crate) async fn panel_context(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> Result<PanelContext, String> {
    let revision_id = entity::workspaces::Entity::find_by_id(workspace_id)
        .one(db)
        .await
        .map_err(|e| e.to_string())?
        .and_then(|w| w.current_revision_id)
        .ok_or("the workspace has no promoted revision")?;
    Ok(PanelContext {
        workspace_id,
        revision_id,
        requested_by: Uuid::nil(),
    })
}

#[cfg(test)]
mod tests {
    use agentic_core::delegation::TaskSpec;
    use serde_json::json;
    use uuid::Uuid;

    use super::*;

    #[test]
    fn the_payload_names_the_run_and_nothing_else_is_accepted() {
        let id = Uuid::new_v4();
        let spec = TaskSpec::Custom {
            kind: CHECK_KIND.into(),
            payload: json!({ "run_id": id }),
        };
        assert_eq!(payload_run_id(&spec), Ok(id));
        let other = TaskSpec::Custom {
            kind: "experiment_estimate".into(),
            payload: json!({ "run_id": id }),
        };
        assert!(
            payload_run_id(&other)
                .unwrap_err()
                .contains("experiment_estimate")
        );
        let empty = TaskSpec::Custom {
            kind: CHECK_KIND.into(),
            payload: json!({}),
        };
        assert!(payload_run_id(&empty).unwrap_err().contains("run_id"));
    }
}
