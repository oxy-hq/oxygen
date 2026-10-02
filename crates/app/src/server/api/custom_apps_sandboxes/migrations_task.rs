//! A sandbox build's Airhouse migrations: the `custom_app_sandbox_migrations`
//! task a publish to a sandbox queues, run on the worker fleet.
//!
//! A publish that names a sandbox moves that sandbox's pointer, so the
//! sandbox's Airhouse sibling (`app_<writer>__dev_<handle>`) must get the
//! build's tables. As for staging (`custom_apps_nonproduction::staging_task`),
//! the apply can take minutes, so the publish queues it as the last thing it
//! does and answers; a task that cannot be queued is a warning on the publish
//! response, and one that fails is its run's failure — never a failed publish.
//!
//! **Its own task kind, not a field on the staging task.** A worker from
//! before sandboxes existed fails a kind it does not know; handed a staging
//! task with a field it ignores, it would apply a sandbox's files to
//! staging's sibling.
//!
//! **Airhouse only.** A sandbox publish applies no OLTP migration: every
//! sandbox, and staging, of every app in the org share the org's one OLTP
//! staging branch, and its ledger refuses a file whose bytes changed — an
//! agent iterating on a migration would break staging for the whole org.
//!
//! **Once per (app, sandbox, build).** The run id is derived from the three.
//!
//! **Nothing is applied for a sandbox that is gone, or has moved on.** The
//! task holds the sandbox's lock ([`super::lock`]) for its whole run — the
//! lock a teardown holds — and reads the sandbox's row under it. Deleted or
//! being deleted since the publish, its sibling is the teardown's to drop,
//! and applying would create it again with nothing left to remove it. Serving
//! another build — a later publish, or a sandbox created again under the name
//! — this build's files are not the sandbox's any more; the build it serves
//! queued a task of its own.

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
use crate::server::api::custom_apps_env_resolve::sandbox_row;
use crate::server::api::custom_apps_migrations::{
    AirhouseRun, Applied, DeclaredMigration, MigrationError, apply_airhouse_to_environment,
};
use crate::server::api::custom_apps_nonproduction::staging_task::QueuedMigration;
use crate::server::api::custom_apps_nonproduction::staging_task_executor::{
    BUSY_RETRY_DELAYS, HUNG_APPLY_BACKSTOP, bounded, retry_busy, staging_migration_deadline,
};

/// The `TaskSpec::Custom` kind, and the run's `source_type` — a platform
/// daemon's, so the coordinator feed and workspace health both leave it out.
pub const SANDBOX_MIGRATIONS_KIND: &str = "custom_app_sandbox_migrations";

/// The payload of one sandbox migration task. The SQL rides in it: a build's
/// files are not kept anywhere a worker could re-read them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxMigrationsTask {
    pub app_id: Uuid,
    pub app_slug: String,
    /// The app's workspace: where the run is filed, and whose Airhouse the
    /// sibling schema lives in.
    pub workspace_id: Uuid,
    pub build_pk: Uuid,
    /// The sandbox's full name, `dev-<handle>`.
    pub environment: String,
    pub migrations: Vec<QueuedMigration>,
}

impl SandboxMigrationsTask {
    /// The run (and task) id: one per (app, sandbox, build).
    pub fn run_id(&self) -> String {
        format!(
            "{SANDBOX_MIGRATIONS_KIND}:{}:{}:{}",
            self.app_id, self.environment, self.build_pk
        )
    }

    /// The sandbox the task names; a payload naming a fixed environment, or
    /// nothing `AppEnvironment::parse` accepts, is refused — staging's sibling
    /// is the staging task's.
    pub fn sandbox(&self) -> Result<AppEnvironment, String> {
        match AppEnvironment::parse(&self.environment) {
            Some(environment @ AppEnvironment::Dev { .. }) => Ok(environment),
            _ => Err(format!(
                "{:?} is not a sandbox; nothing was applied",
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
            kind: SANDBOX_MIGRATIONS_KIND.to_string(),
            payload: serde_json::to_value(self)?,
        })
    }

    /// The payload of a queued spec.
    pub fn from_spec(spec: &TaskSpec) -> Result<Self, String> {
        match spec {
            TaskSpec::Custom { kind, payload } if kind == SANDBOX_MIGRATIONS_KIND => {
                serde_json::from_value(payload.clone())
                    .map_err(|e| format!("bad sandbox migration payload: {e}"))
            }
            other => Err(format!("not a sandbox migration task: {other:?}")),
        }
    }
}

/// Queue `task` unless its run already exists. `Ok(true)`: this call queued
/// it; `Ok(false)`: an earlier one had. Exactly as `staging_task::enqueue`.
pub async fn enqueue(db: &DatabaseConnection, task: &SandboxMigrationsTask) -> Result<bool, DbErr> {
    let run_id = task.run_id();
    let spec = task.spec().map_err(|e| DbErr::Custom(e.to_string()))?;
    let txn = db.begin().await?;
    let inserted = agentic_runtime::crud::insert_run(
        &txn,
        &run_id,
        &format!(
            "Migrate sandbox {}'s Airhouse schema for {}",
            task.environment, task.app_slug
        ),
        None,
        SANDBOX_MIGRATIONS_KIND,
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
    tracing::info!(app_id = %task.app_id, %run_id, "publish: sandbox migrations queued");
    Ok(true)
}

pub struct SandboxMigrationsExecutor {
    pub db: DatabaseConnection,
}

#[async_trait]
impl TaskExecutor for SandboxMigrationsExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let task = SandboxMigrationsTask::from_spec(&assignment.spec)?;
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
async fn run_guarded(db: &DatabaseConnection, task: &SandboxMigrationsTask) -> TaskOutcome {
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
                environment = %task.environment, "{SANDBOX_MIGRATIONS_KIND}: {warning}");
            TaskOutcome::Failed(warning)
        }
        Err(_) => {
            tracing::error!(app_id = %task.app_id, environment = %task.environment,
                "{SANDBOX_MIGRATIONS_KIND} panicked");
            TaskOutcome::Failed("the sandbox migration panicked".to_string())
        }
    }
}

/// Apply `task` to the sandbox's sibling: `Ok(what was applied)`, or
/// `Err(the warning)`. Waits [`BUSY_RETRY_DELAYS`] for the sandbox's lock;
/// see [`run_with`].
pub async fn run(db: &DatabaseConnection, task: &SandboxMigrationsTask) -> Result<String, String> {
    run_with(db, task, &BUSY_RETRY_DELAYS).await
}

/// [`run`], trying the sandbox's lock again after each of `lock_waits`. A
/// sandbox a teardown or another apply still holds after the last gets
/// nothing applied: `Err`.
pub async fn run_with(
    db: &DatabaseConnection,
    task: &SandboxMigrationsTask,
    lock_waits: &[Duration],
) -> Result<String, String> {
    let environment = task.sandbox()?;
    let lock = SandboxLock::acquire(db, task.app_id, &task.environment, lock_waits)
        .await
        .map_err(|e| not_migrated(task, format!("take its lock: {e}")))?;
    let Some(lock) = lock else {
        let busy = "a teardown or another migration of it is still running";
        return Err(not_migrated(task, busy.to_string()));
    };
    let outcome = migrate(db, task, &environment).await;
    lock.release().await;
    outcome
}

fn not_migrated(task: &SandboxMigrationsTask, why: String) -> String {
    format!(
        "sandbox {}'s Airhouse schema was not migrated ({why}); the publish went on, and the \
         sandbox's Airhouse writes will fail until a later publish to it migrates it",
        task.environment
    )
}

/// Everything that runs under the sandbox's lock.
async fn migrate(
    db: &DatabaseConnection,
    task: &SandboxMigrationsTask,
    environment: &AppEnvironment,
) -> Result<String, String> {
    let name = &task.environment;
    let row = sandbox_row(db, task.app_id, environment)
        .await
        .map_err(|e| not_migrated(task, format!("read its row: {e}")))?;
    let Some(row) = row else {
        return Ok(format!(
            "sandbox {name} was deleted since the publish; nothing was applied"
        ));
    };
    if row.build_id != Some(task.build_pk) {
        return Ok(format!(
            "sandbox {name} no longer serves this build; nothing was applied (the build it \
             serves queued its own migrations)"
        ));
    }
    let declared = task.declared();
    let deadline = staging_migration_deadline();
    let attempt = || attempt(db, task, &declared, environment, deadline);
    let why = match retry_busy(&BUSY_RETRY_DELAYS, attempt).await {
        Ok(applied) if applied.deferred.is_empty() => return Ok(summary(&applied)),
        Ok(applied) => format!(
            "its deadline passed between files; {} left for the next publish",
            applied.deferred.join(", ")
        ),
        Err(e) => e.to_string(),
    };
    Err(not_migrated(task, why))
}

async fn attempt(
    db: &DatabaseConnection,
    task: &SandboxMigrationsTask,
    declared: &[DeclaredMigration],
    environment: &AppEnvironment,
    deadline: Duration,
) -> Result<Applied, MigrationError> {
    let run = AirhouseRun {
        app_id: task.app_id,
        app_slug: &task.app_slug,
        workspace_id: task.workspace_id,
        build_pk: task.build_pk,
        start_files_until: Some(tokio::time::Instant::now() + deadline),
    };
    let apply = apply_airhouse_to_environment(db, run, declared, environment);
    let backstop = deadline + HUNG_APPLY_BACKSTOP;
    bounded(backstop, apply).await.unwrap_or_else(|| {
        Err(MigrationError::Infra {
            filename: String::new(),
            message: format!("it passed its {}s deadline", backstop.as_secs()),
        })
    })
}

fn summary(applied: &Applied) -> String {
    match applied.summary() {
        s if s.is_empty() => "nothing to apply".to_string(),
        s => s,
    }
}

#[cfg(test)]
#[path = "migrations_task_tests.rs"]
mod tests;
