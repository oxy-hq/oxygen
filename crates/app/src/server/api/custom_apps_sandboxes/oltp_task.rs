//! A sandbox's own OLTP schema, made and migrated: the
//! `custom_app_sandbox_oltp` task a publish to a sandbox queues, run on the
//! worker fleet (`internal-docs/per-org-oltp-postgres.md` → Sandbox schemas
//! on the staging branch).
//!
//! In an org with an OLTP staging branch, a sandbox's `ctx.oltp` runs in a
//! schema of its own inside that branch. This task is the only thing that
//! creates one. It:
//!
//! 1. **creates and seeds** the schema from staging's, when the sandbox's row
//!    does not record one ready on the branch's current cut — a first
//!    publish, a seed that failed, or a branch reset since
//!    ([`super::oltp_home::ensure`]);
//! 2. **applies the build's OLTP migrations** to it, under the sandbox's own
//!    ledger target — only the files staging has not applied.
//!
//! One task for both, in that order: the build's files must land on the
//! seeded copy, and two tasks would race.
//!
//! **Created by a publish, not by `env create`.** A sandbox with no build
//! runs nothing; the publish is the only moment the build's migrations are in
//! hand; and it is the one action that repairs a sandbox after a reset.
//!
//! **Its own task kind**, as the sandbox Airhouse task is
//! (`migrations_task`): a worker from before this existed fails a kind it
//! does not know, where a field on the staging task would have it migrate
//! staging's schema.
//!
//! **Never a failed publish.** A task that cannot be queued is a warning on
//! the publish response; one that fails is its run's failure, and — for a
//! failed seed — the row's `failed` state, which `oxyc env show` reports and
//! the sandbox's `ctx.oltp` is refused with.
//!
//! **Nothing is done for a sandbox that is gone, or has moved on**, exactly
//! as for its Airhouse migrations: the task holds the sandbox's lock
//! ([`super::lock`]) — the lock a teardown holds — and reads the row under
//! it. Deleted or being deleted, the schema is the teardown's to drop;
//! serving another build, that build queued a task of its own.

use std::time::Duration;

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::orchestrator::crud::queue::TaskScope;
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use futures::FutureExt;
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{DatabaseConnection, DbErr, SqlErr, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::lock::SandboxLock;
use super::oltp_home;
use crate::server::api::custom_apps_env_resolve::sandbox_row;
use crate::server::api::custom_apps_migrations::DeclaredMigration;
use crate::server::api::custom_apps_nonproduction::staging_task::QueuedMigration;
use crate::server::api::custom_apps_nonproduction::staging_task_executor::BUSY_RETRY_DELAYS;

/// The `TaskSpec::Custom` kind, and the run's `source_type` — a platform
/// daemon's, so the coordinator feed and workspace health both leave it out.
pub const SANDBOX_OLTP_KIND: &str = "custom_app_sandbox_oltp";

/// The payload of one sandbox OLTP task. The SQL rides in it: a build's files
/// are not kept anywhere a worker could re-read them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxOltpTask {
    pub app_id: Uuid,
    pub app_slug: String,
    /// Whose OLTP staging branch the schema lives in.
    pub org_id: Uuid,
    /// The app's workspace: where the run is filed.
    pub workspace_id: Uuid,
    pub build_pk: Uuid,
    /// The sandbox's full name, `dev-<handle>`.
    pub environment: String,
    /// The build's OLTP migrations; empty for a build that declares none,
    /// whose sandbox still needs its schema.
    pub migrations: Vec<QueuedMigration>,
}

impl SandboxOltpTask {
    /// The run (and task) id: one per (app, sandbox, build).
    pub fn run_id(&self) -> String {
        format!(
            "{SANDBOX_OLTP_KIND}:{}:{}:{}",
            self.app_id, self.environment, self.build_pk
        )
    }

    /// The sandbox the task names; a payload naming a fixed environment, or
    /// nothing `AppEnvironment::parse` accepts, is refused — staging's schema
    /// is the staging task's.
    pub fn sandbox(&self) -> Result<AppEnvironment, String> {
        match AppEnvironment::parse(&self.environment) {
            Some(environment @ AppEnvironment::Dev { .. }) => Ok(environment),
            _ => Err(format!(
                "{:?} is not a sandbox; nothing was created or applied",
                self.environment
            )),
        }
    }

    pub fn declared(&self) -> Vec<DeclaredMigration> {
        self.migrations
            .iter()
            .map(|m| DeclaredMigration {
                filename: m.filename.clone(),
                checksum: m.checksum.clone(),
                sql: m.sql.clone(),
            })
            .collect()
    }

    pub fn spec(&self) -> Result<TaskSpec, serde_json::Error> {
        Ok(TaskSpec::Custom {
            kind: SANDBOX_OLTP_KIND.to_string(),
            payload: serde_json::to_value(self)?,
        })
    }

    /// The payload of a queued spec.
    pub fn from_spec(spec: &TaskSpec) -> Result<Self, String> {
        match spec {
            TaskSpec::Custom { kind, payload } if kind == SANDBOX_OLTP_KIND => {
                serde_json::from_value(payload.clone())
                    .map_err(|e| format!("bad sandbox OLTP payload: {e}"))
            }
            other => Err(format!("not a sandbox OLTP task: {other:?}")),
        }
    }
}

/// Queue `task` unless its run already exists. `Ok(true)`: this call queued
/// it; `Ok(false)`: an earlier one had. Exactly as `migrations_task::enqueue`.
pub async fn enqueue(db: &DatabaseConnection, task: &SandboxOltpTask) -> Result<bool, DbErr> {
    let run_id = task.run_id();
    let spec = task.spec().map_err(|e| DbErr::Custom(e.to_string()))?;
    let txn = db.begin().await?;
    let inserted = agentic_runtime::crud::insert_run(
        &txn,
        &run_id,
        &format!(
            "Prepare sandbox {}'s OLTP schema for {}",
            task.environment, task.app_slug
        ),
        None,
        SANDBOX_OLTP_KIND,
        Some(json!({
            "app_id": task.app_id,
            "app_slug": task.app_slug,
            "build_pk": task.build_pk,
            "environment": task.environment,
        })),
        task.workspace_id,
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
    tracing::info!(app_id = %task.app_id, %run_id, "publish: sandbox OLTP schema queued");
    Ok(true)
}

pub struct SandboxOltpExecutor {
    pub db: DatabaseConnection,
}

#[async_trait]
impl TaskExecutor for SandboxOltpExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let task = SandboxOltpTask::from_spec(&assignment.spec)?;
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
async fn run_guarded(db: &DatabaseConnection, task: &SandboxOltpTask) -> TaskOutcome {
    let run = std::panic::AssertUnwindSafe(run(db, task))
        .catch_unwind()
        .await;
    let metadata = json!({
        "app_id": task.app_id,
        "build_pk": task.build_pk,
        "environment": task.environment,
    });
    match run {
        Ok(Ok(summary)) => TaskOutcome::Done {
            answer: summary,
            metadata: Some(metadata),
        },
        Ok(Err(warning)) => {
            tracing::warn!(app_id = %task.app_id, build_pk = %task.build_pk,
                environment = %task.environment, "{SANDBOX_OLTP_KIND}: {warning}");
            TaskOutcome::Failed(warning)
        }
        Err(_) => {
            tracing::error!(app_id = %task.app_id, environment = %task.environment,
                "{SANDBOX_OLTP_KIND} panicked");
            TaskOutcome::Failed("the sandbox OLTP task panicked".to_string())
        }
    }
}

/// Make the sandbox's schema usable and apply `task`'s files to it:
/// `Ok(what was done)`, or `Err(the warning)`. Waits [`BUSY_RETRY_DELAYS`]
/// for the sandbox's lock; see [`run_with`].
pub async fn run(db: &DatabaseConnection, task: &SandboxOltpTask) -> Result<String, String> {
    run_with(db, task, &BUSY_RETRY_DELAYS).await
}

/// [`run`], trying the sandbox's lock again after each of `lock_waits`. A
/// sandbox a teardown or another task still holds after the last gets
/// nothing done: `Err`.
pub async fn run_with(
    db: &DatabaseConnection,
    task: &SandboxOltpTask,
    lock_waits: &[Duration],
) -> Result<String, String> {
    let environment = task.sandbox()?;
    let lock = SandboxLock::acquire(db, task.app_id, &task.environment, lock_waits)
        .await
        .map_err(|e| not_prepared(task, format!("take its lock: {e}")))?;
    let Some(lock) = lock else {
        let busy = "a teardown or another task of it is still running";
        return Err(not_prepared(task, busy.to_string()));
    };
    let outcome = prepare(db, task, &environment).await;
    lock.release().await;
    outcome
}

fn not_prepared(task: &SandboxOltpTask, why: String) -> String {
    format!(
        "sandbox {}'s OLTP schema is not ready ({why}); the publish went on, and the sandbox's \
         ctx.oltp may be refused or miss this build's tables until a later publish to it succeeds",
        task.environment
    )
}

/// Everything that runs under the sandbox's lock.
async fn prepare(
    db: &DatabaseConnection,
    task: &SandboxOltpTask,
    environment: &AppEnvironment,
) -> Result<String, String> {
    let name = &task.environment;
    let row = sandbox_row(db, task.app_id, environment)
        .await
        .map_err(|e| not_prepared(task, format!("read its row: {e}")))?;
    let Some(row) = row else {
        return Ok(format!(
            "sandbox {name} was deleted since the publish; nothing was created or applied"
        ));
    };
    if row.build_id != Some(task.build_pk) {
        return Ok(format!(
            "sandbox {name} no longer serves this build; nothing was created or applied (the \
             build it serves queued its own task)"
        ));
    }
    let sandbox = oltp_home::Sandbox {
        app_id: task.app_id,
        app_slug: &task.app_slug,
        org_id: task.org_id,
        environment,
    };
    let seeded = oltp_home::ensure(db, sandbox, &row)
        .await
        .map_err(|why| not_prepared(task, why))?;
    let Some(seeded) = seeded else {
        return Ok(format!(
            "the org has no active OLTP staging branch; sandbox {name}'s ctx.oltp writes stay held"
        ));
    };
    let applied = oltp_home::migrate(db, sandbox, task.build_pk, &task.declared())
        .await
        .map_err(|why| not_prepared(task, why))?;
    Ok(format!("{seeded}; {applied}"))
}

#[cfg(test)]
#[path = "oltp_task_tests.rs"]
mod tests;
