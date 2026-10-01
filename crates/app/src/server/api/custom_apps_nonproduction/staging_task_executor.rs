//! Runs a queued staging migration ([`super::staging_task`]) on the worker
//! fleet. Registered for [`STAGING_MIGRATIONS_KIND`] by
//! `server::router::recovery::build_custom_task_registry`.
//!
//! Each store keeps the bounds and the lock its apply always had:
//!
//! - **Airhouse**: the apply's deadline ([`staging_migration_deadline`],
//!   `OXY_STAGING_AIRHOUSE_MIGRATION_DEADLINE_SECS`, default 5 minutes) is
//!   checked **between files** — past it no file starts, a file in progress
//!   finishes and is recorded — so it never splits a tenant `COMMIT` from its
//!   ledger row. A backstop [`HUNG_APPLY_BACKSTOP`] past the deadline drops an
//!   apply stuck inside one file, which rolls back its lock transaction and
//!   so releases the per-target advisory lock.
//! - **OLTP branch**: session lock and statement timeouts, and a whole-step
//!   timeout (`custom_apps_migrations::branch`).
//!
//! Both take the app's per-target advisory lock, and a lock held by another
//! apply of the same app to the same store — two publishes' tasks side by side
//! — answers `Busy`; that is waited out ([`BUSY_RETRY_DELAYS`]) rather than
//! dropping the newer build's files.
//!
//! The outcome is the record: `Done` with what was applied, or `Failed` with
//! the warning the publish response used to carry. Nothing retries a failed
//! task; the next publish queues its build's apply, which plans the same
//! missing files again.

use std::future::Future;
use std::time::Duration;

use agentic_core::delegation::{TaskAssignment, TaskOutcome};
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use futures::FutureExt;
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::DatabaseConnection;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::staging_task::{STAGING_MIGRATIONS_KIND, StagingMigrationTask, StagingStore};
use crate::server::api::custom_apps_migrations::{
    AirhouseRun, Applied, DeclaredMigration, MigrationError, apply_airhouse_to_environment,
    apply_to_staging_branch, branch_warning,
};

/// The default deadline of a sibling Airhouse apply: no file starts after it.
const DEFAULT_MIGRATION_DEADLINE: Duration = Duration::from_secs(300);

/// How long past its deadline an apply stuck inside one file is dropped.
pub const HUNG_APPLY_BACKSTOP: Duration = Duration::from_secs(600);

/// How long a `Busy` apply waits before each retry; `Busy` after the last
/// fails the task.
pub const BUSY_RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(5),
    Duration::from_secs(20),
    Duration::from_secs(60),
];

/// The deadline of a sibling Airhouse apply:
/// `OXY_STAGING_AIRHOUSE_MIGRATION_DEADLINE_SECS`, else 5 minutes.
pub fn staging_migration_deadline() -> Duration {
    std::env::var("OXY_STAGING_AIRHOUSE_MIGRATION_DEADLINE_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_MIGRATION_DEADLINE)
}

/// `apply`, dropped at `deadline` if it has not finished: `None`. Dropping it
/// ends whatever it holds — a lock transaction rolls back, a connection closes.
pub async fn bounded<F: Future>(deadline: Duration, apply: F) -> Option<F::Output> {
    tokio::time::timeout(deadline, apply).await.ok()
}

pub struct StagingMigrationsExecutor {
    pub db: DatabaseConnection,
}

#[async_trait]
impl TaskExecutor for StagingMigrationsExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let task = StagingMigrationTask::from_spec(&assignment.spec)?;
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
async fn run_guarded(db: &DatabaseConnection, task: &StagingMigrationTask) -> TaskOutcome {
    let run = std::panic::AssertUnwindSafe(run(db, task))
        .catch_unwind()
        .await;
    let metadata = json!({ "app_id": task.app_id, "build_pk": task.build_pk, "store": task.store });
    match run {
        Ok(Ok(summary)) => TaskOutcome::Done {
            answer: summary,
            metadata: Some(metadata),
        },
        Ok(Err(warning)) => {
            tracing::warn!(app_id = %task.app_id, build_pk = %task.build_pk,
                "{STAGING_MIGRATIONS_KIND}: {warning}");
            TaskOutcome::Failed(warning)
        }
        Err(_) => {
            tracing::error!(app_id = %task.app_id, "{STAGING_MIGRATIONS_KIND} panicked");
            TaskOutcome::Failed("the staging migration panicked".to_string())
        }
    }
}

/// Apply `task` to its store: `Ok(what was applied)`, or `Err(the warning)`.
pub async fn run(db: &DatabaseConnection, task: &StagingMigrationTask) -> Result<String, String> {
    let declared = task.declared();
    match task.store {
        StagingStore::Airhouse => migrate_airhouse(db, task, &declared).await,
        StagingStore::OltpBranch => migrate_branch(db, task, &declared).await,
    }
}

async fn migrate_airhouse(
    db: &DatabaseConnection,
    task: &StagingMigrationTask,
    declared: &[DeclaredMigration],
) -> Result<String, String> {
    let deadline = staging_migration_deadline();
    let attempt = || airhouse_attempt(db, task, declared, deadline);
    let why = match retry_busy(&BUSY_RETRY_DELAYS, attempt).await {
        Ok(applied) if applied.deferred.is_empty() => return Ok(summary(&applied)),
        Ok(applied) => format!(
            "its deadline passed between files; {} left for the next publish",
            applied.deferred.join(", ")
        ),
        Err(e) => e.to_string(),
    };
    Err(format!(
        "staging's Airhouse schema was not migrated ({why}); the publish went on, and staging's \
         Airhouse writes will fail until a later publish migrates it"
    ))
}

async fn airhouse_attempt(
    db: &DatabaseConnection,
    task: &StagingMigrationTask,
    declared: &[DeclaredMigration],
    deadline: Duration,
) -> Result<Applied, MigrationError> {
    let run = AirhouseRun {
        app_id: task.app_id,
        app_slug: &task.app_slug,
        workspace_id: task.workspace_id,
        build_pk: task.build_pk,
        start_files_until: Some(tokio::time::Instant::now() + deadline),
    };
    let apply = apply_airhouse_to_environment(db, run, declared, &AppEnvironment::Staging);
    let backstop = deadline + HUNG_APPLY_BACKSTOP;
    bounded(backstop, apply).await.unwrap_or_else(|| {
        Err(MigrationError::Infra {
            filename: String::new(),
            message: format!("it passed its {}s deadline", backstop.as_secs()),
        })
    })
}

async fn migrate_branch(
    db: &DatabaseConnection,
    task: &StagingMigrationTask,
    declared: &[DeclaredMigration],
) -> Result<String, String> {
    let attempt = || {
        apply_to_staging_branch(
            db,
            task.app_id,
            &task.app_slug,
            task.org_id,
            task.build_pk,
            declared,
        )
    };
    match retry_busy(&BUSY_RETRY_DELAYS, attempt).await {
        Ok(Some(applied)) => Ok(summary(&applied)),
        Ok(None) => Ok("the org has no OLTP staging branch".to_string()),
        Err(e) => Err(branch_warning(&e)),
    }
}

fn summary(applied: &Applied) -> String {
    match applied.summary() {
        s if s.is_empty() => "nothing to apply".to_string(),
        s => s,
    }
}

/// Run `attempt`, again after each of `delays` while it answers `Busy`.
pub async fn retry_busy<T, F, Fut>(delays: &[Duration], mut attempt: F) -> Result<T, MigrationError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, MigrationError>>,
{
    for delay in delays {
        match attempt().await {
            Err(MigrationError::Busy) => tokio::time::sleep(*delay).await,
            other => return other,
        }
    }
    attempt().await
}

#[cfg(test)]
#[path = "staging_task_executor_tests.rs"]
mod tests;
