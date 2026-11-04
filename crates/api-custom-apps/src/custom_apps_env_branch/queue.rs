//! The queue side of the `custom_app_env_oltp_branch` task: what a queued run
//! carries ([`EnvBranchTask`], [`BranchOp`]), how it is queued ([`enqueue`]) and
//! the executor the worker fleet registers ([`EnvBranchExecutor`]). What the
//! run does to the branch is `task`; `task` re-exports these, so their public
//! paths are `custom_apps_env_branch::task::*`.

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::orchestrator::crud::queue::TaskScope;
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::FutureExt;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_oltp::env_branches::EnvBranchKey;
use std::collections::HashSet;

use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, SqlErr, Statement,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::task::run;

/// The `TaskSpec::Custom` kind, and the run's `source_type` — a platform
/// daemon's, so the coordinator feed and workspace health both leave it out.
pub const ENV_OLTP_BRANCH_KIND: &str = "custom_app_env_oltp_branch";

/// What the task does to the environment's branch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum BranchOp {
    /// Cut it (or finish a cut that failed), forking production at `at`
    /// (`None`: now).
    Cut { at: Option<DateTime<Utc>> },
    /// Throw its data away and cut it again, at `at` (`None`: from the parent
    /// as it is now).
    Reset { at: Option<DateTime<Utc>> },
    /// Apply build `build_pk`'s OLTP files to the branch as it is.
    Migrate { build_pk: Uuid },
    /// Delete the branch at the provider and its row: the sandbox's planes no
    /// longer ask for one. A no-op if they ask for one again by the time it
    /// runs, so a switch back never loses the branch the second switch cut.
    Drop,
}

impl BranchOp {
    /// The op's name, as it appears in the run id.
    pub fn tag(&self) -> &'static str {
        match self {
            BranchOp::Cut { .. } => "cut",
            BranchOp::Reset { .. } => "reset",
            BranchOp::Migrate { .. } => "migrate",
            BranchOp::Drop => "drop",
        }
    }
}

/// The payload of one environment-branch task.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvBranchTask {
    pub app_id: Uuid,
    pub app_slug: String,
    /// Whose OLTP database the branch is cut from.
    pub org_id: Uuid,
    /// The app's workspace: where the run is filed.
    pub workspace_id: Uuid,
    /// The sandbox's full name, `dev-<handle>`.
    pub environment: String,
    pub op: BranchOp,
}

impl EnvBranchTask {
    /// A run id for one enqueue: `{KIND}:{app}:{env}:{op}:{nonce}`. The nonce
    /// is fresh on every call, so a reset asked for twice runs twice — a run id
    /// that repeated would have the second one dropped as already queued. Call
    /// it once per task; [`enqueue`] does.
    pub fn run_id(&self) -> String {
        format!(
            "{ENV_OLTP_BRANCH_KIND}:{}:{}:{}:{}",
            self.app_id,
            self.environment,
            self.op.tag(),
            Uuid::new_v4().simple()
        )
    }

    /// The sandbox the task names; a payload naming a fixed environment, or
    /// nothing `AppEnvironment::parse` accepts, is refused.
    pub fn sandbox(&self) -> Result<AppEnvironment, String> {
        match AppEnvironment::parse(&self.environment) {
            Some(environment @ AppEnvironment::Dev { .. }) => Ok(environment),
            _ => Err(format!(
                "{:?} is not a sandbox; nothing was done to its OLTP branch",
                self.environment
            )),
        }
    }

    pub(super) fn key(&self) -> EnvBranchKey {
        EnvBranchKey {
            org_id: self.org_id,
            app_id: self.app_id,
            env_name: self.environment.clone(),
        }
    }

    pub fn spec(&self) -> Result<TaskSpec, serde_json::Error> {
        Ok(TaskSpec::Custom {
            kind: ENV_OLTP_BRANCH_KIND.to_string(),
            payload: serde_json::to_value(self)?,
        })
    }

    /// The payload of a queued spec.
    pub fn from_spec(spec: &TaskSpec) -> Result<Self, String> {
        match spec {
            TaskSpec::Custom { kind, payload } if kind == ENV_OLTP_BRANCH_KIND => {
                serde_json::from_value(payload.clone())
                    .map_err(|e| format!("bad environment branch payload: {e}"))
            }
            other => Err(format!("not an environment branch task: {other:?}")),
        }
    }

    pub(super) fn metadata(&self) -> serde_json::Value {
        json!({
            "app_id": self.app_id,
            "app_slug": self.app_slug,
            "environment": self.environment,
            "op": self.op.tag(),
        })
    }
}

/// Queue `task`. `Ok(true)`: this call queued it; `Ok(false)`: its run id
/// already existed (it does not, with a fresh nonce, unless a caller reuses
/// one).
pub async fn enqueue(db: &DatabaseConnection, task: &EnvBranchTask) -> Result<bool, DbErr> {
    let txn = db.begin().await?;
    match enqueue_in(&txn, task).await {
        Ok(_) => {}
        Err(e) if matches!(e.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) => {
            txn.rollback().await?;
            return Ok(false);
        }
        Err(e) => return Err(e),
    }
    txn.commit().await?;
    Ok(true)
}

/// [`enqueue`], answering the queued run's id.
pub async fn enqueue_run(db: &DatabaseConnection, task: &EnvBranchTask) -> Result<String, DbErr> {
    let txn = db.begin().await?;
    let run_id = enqueue_in(&txn, task).await?;
    txn.commit().await?;
    Ok(run_id)
}

/// Queue `task` on `conn`, which is a transaction when the queue entry must
/// stand or fall with the write that asked for it (a change of planes, a
/// reset's fork point). The queued run's id.
pub async fn enqueue_in<C: ConnectionTrait>(
    conn: &C,
    task: &EnvBranchTask,
) -> Result<String, DbErr> {
    let run_id = task.run_id();
    let spec = task.spec().map_err(|e| DbErr::Custom(e.to_string()))?;
    agentic_runtime::crud::insert_run(
        conn,
        &run_id,
        &format!(
            "{} sandbox {}'s OLTP branch for {}",
            task.op.tag(),
            task.environment,
            task.app_slug
        ),
        None,
        ENV_OLTP_BRANCH_KIND,
        Some(task.metadata()),
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
    .await?;
    tracing::info!(app_id = %task.app_id, %run_id, "environment OLTP branch task queued");
    Ok(run_id)
}

/// The sandboxes of `app_id` with a cut or reset of their branch waiting or
/// running on the queue. A queued re-cut leaves the branch `ready` until the
/// worker takes it, so the branch row alone cannot say one is on its way.
pub async fn recuts_queued<C: ConnectionTrait>(
    conn: &C,
    app_id: Uuid,
) -> Result<HashSet<String>, DbErr> {
    let rows = conn
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT DISTINCT spec->'payload'->>'environment' AS environment \
               FROM agentic_task_queue \
              WHERE queue_status IN ('queued', 'claimed') AND spec->>'kind' = $1 \
                AND spec->'payload'->>'app_id' = $2 \
                AND spec->'payload'->'op'->>'op' IN ('cut', 'reset')",
            [ENV_OLTP_BRANCH_KIND.into(), app_id.to_string().into()],
        ))
        .await?;
    rows.iter()
        .map(|row| row.try_get("", "environment"))
        .collect()
}

pub struct EnvBranchExecutor {
    pub db: DatabaseConnection,
}

#[async_trait]
impl TaskExecutor for EnvBranchExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let task = EnvBranchTask::from_spec(&assignment.spec)?;
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
async fn run_guarded(db: &DatabaseConnection, task: &EnvBranchTask) -> TaskOutcome {
    let run = std::panic::AssertUnwindSafe(run(db, task))
        .catch_unwind()
        .await;
    match run {
        Ok(Ok(summary)) => TaskOutcome::Done {
            answer: summary,
            metadata: Some(task.metadata()),
        },
        Ok(Err(warning)) => {
            tracing::warn!(app_id = %task.app_id, environment = %task.environment,
                op = task.op.tag(), "{ENV_OLTP_BRANCH_KIND}: {warning}");
            TaskOutcome::Failed(warning)
        }
        Err(_) => {
            tracing::error!(app_id = %task.app_id, environment = %task.environment,
                "{ENV_OLTP_BRANCH_KIND} panicked");
            TaskOutcome::Failed("the environment OLTP branch task panicked".to_string())
        }
    }
}
