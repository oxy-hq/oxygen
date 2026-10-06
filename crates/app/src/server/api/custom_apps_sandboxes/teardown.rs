//! The second half of deleting a sandbox: the `custom_app_sandbox_teardown`
//! task, run on the worker fleet (`internal-docs/custom-app-sandboxes.md` →
//! Lifecycle).
//!
//! `ops::begin_delete` marks the row `deleting_at`, clears its pointer and
//! queues this task in one transaction, so the sandbox stops serving at once
//! and the request never waits on an object store or on Airhouse. The task
//! then removes the sandbox's homes — its storage silo, its secrets, its
//! Airhouse sibling and its own schema on the org's OLTP staging branch —
//! and, last, the row, which is what frees the name.
//!
//! **Each step is idempotent**, and the row goes last: a run that fails in
//! storage, the secret store, Airhouse or the OLTP branch leaves the row
//! `deleting`, so the name stays taken and a later run (a second `DELETE`, or
//! the maintenance loop after six hours) finishes what is left. The OLTP
//! step fails — and so keeps the row — whenever the row records a schema and
//! its drop cannot be confirmed.
//!
//! **One run of a sandbox at a time, and only of a sandbox marked for it.**
//! Two runs of one sandbox can exist — a task claimed again after its worker
//! was given up on, a retry queued once the run before it had failed. Were
//! one to finish and the name be created again while the other was still
//! working, the other would remove the new sandbox's homes. So a run holds
//! the sandbox's lock ([`super::lock`]) for its whole duration and reads the
//! row under it, and goes on only when the row is there and marked
//! `deleting`. A row that is **absent while the app exists** is a sandbox an
//! earlier run finished: whatever is under the name now is not this run's to
//! remove, and it ends having removed nothing — as it does for a row that is
//! there and not marked, a newer sandbox. Only when the app itself is gone
//! (its rows went with it, its homes did not) does a run go on without a row.

use agentic_core::delegation::TaskSpec;
use agentic_runtime::orchestrator::crud::queue::TaskScope;
use entity::{app_environments, apps};
use oxy_app_core::custom_app_environment::{AppEnvironment, AppEnvironmentKind};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, EntityTrait,
    QueryFilter, QuerySelect, Statement,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;
use uuid::Uuid;

use super::lock::SandboxLock;
pub use super::teardown_executor::SandboxTeardownExecutor;
use super::{oltp_home, oltp_state};
use crate::server::api::custom_apps_migrations::{
    AirhouseDrop, DropOutcome, MigrationError, SandboxOltpDrop, drop_environment_schema,
};
use crate::server::api::custom_apps_nonproduction::staging_task_executor::{
    BUSY_RETRY_DELAYS, HUNG_APPLY_BACKSTOP, bounded, retry_busy,
};
use crate::server::api::custom_apps_secrets::delete_environment_secrets;
use crate::server::api::custom_apps_storage::delete_environment_assets;

/// The `TaskSpec::Custom` kind, and the run's `source_type` — a platform
/// daemon's, so the coordinator feed and workspace health both leave it out.
pub const SANDBOX_TEARDOWN_KIND: &str = "custom_app_sandbox_teardown";

/// The payload of one teardown. Everything the task needs: it never reads the
/// `apps` row, so it still runs when the app itself has since been deleted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxTeardownTask {
    pub app_id: Uuid,
    pub app_slug: String,
    pub org_id: Uuid,
    /// The app's workspace: where the run is filed, whose secret store holds
    /// the sandbox's secrets, and whose Airhouse its sibling lives in.
    pub workspace_id: Uuid,
    /// The sandbox's full name, `dev-<handle>`.
    pub environment: String,
    /// `deleted`, `expired`, `retried` or `token_ended` (`TeardownReason::as_str`).
    pub reason: String,
    /// When the row was marked, in microseconds since the epoch: what tells
    /// one request to delete from the next, and so one run from the next.
    pub marked_at_micros: i64,
}

impl SandboxTeardownTask {
    /// The run (and task) id: one per time the sandbox was marked.
    pub fn run_id(&self) -> String {
        run_id_of(self.app_id, &self.environment, self.marked_at_micros)
    }

    /// The sandbox the task names. A payload naming production or staging —
    /// or nothing `AppEnvironment::parse` accepts — is refused: a teardown
    /// removes a sandbox's homes, never a fixed environment's.
    pub fn sandbox(&self) -> Result<AppEnvironment, String> {
        match AppEnvironment::parse(&self.environment) {
            Some(environment @ AppEnvironment::Dev { .. }) => Ok(environment),
            _ => Err(format!(
                "{:?} is not a sandbox; nothing was removed",
                self.environment
            )),
        }
    }

    pub fn spec(&self) -> Result<TaskSpec, serde_json::Error> {
        Ok(TaskSpec::Custom {
            kind: SANDBOX_TEARDOWN_KIND.to_string(),
            payload: serde_json::to_value(self)?,
        })
    }

    /// The payload of a queued spec.
    pub fn from_spec(spec: &TaskSpec) -> Result<Self, String> {
        match spec {
            TaskSpec::Custom { kind, payload } if kind == SANDBOX_TEARDOWN_KIND => {
                serde_json::from_value(payload.clone())
                    .map_err(|e| format!("bad sandbox teardown payload: {e}"))
            }
            other => Err(format!("not a sandbox teardown task: {other:?}")),
        }
    }
}

/// The id of the run queued when `environment` of `app_id` was marked at
/// `marked_at_micros` — derivable from the row, so a delete can tell whether
/// the run it queued last is still on its way.
pub fn run_id_of(app_id: Uuid, environment: &str, marked_at_micros: i64) -> String {
    format!("{SANDBOX_TEARDOWN_KIND}:{app_id}:{environment}:{marked_at_micros}")
}

/// Whether the queue still holds `run_id`'s task waiting or running. A task
/// that ended (done, failed, dead-lettered, cancelled) or was purged is not
/// pending: a sandbox still `deleting` after it needs a run of its own.
pub(super) async fn is_pending<C: ConnectionTrait>(conn: &C, run_id: &str) -> Result<bool, DbErr> {
    let row = conn
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT 1 AS pending FROM agentic_task_queue \
              WHERE task_id = $1 AND queue_status IN ('queued', 'claimed')",
            [run_id.into()],
        ))
        .await?;
    Ok(row.is_some())
}

/// Seed `task`'s run and queue it, on `conn` — the transaction that marked the
/// row, so a sandbox is never marked without its teardown queued, nor queued
/// without being marked. Exactly as `staging_task::enqueue` files its run.
pub(super) async fn enqueue<C: ConnectionTrait>(
    conn: &C,
    task: &SandboxTeardownTask,
) -> Result<String, DbErr> {
    let run_id = task.run_id();
    let spec = task.spec().map_err(|e| DbErr::Custom(e.to_string()))?;
    agentic_runtime::crud::insert_run(
        conn,
        &run_id,
        &format!(
            "Tear down sandbox {} of {}",
            task.environment, task.app_slug
        ),
        None,
        SANDBOX_TEARDOWN_KIND,
        Some(json!({
            "app_id": task.app_id,
            "app_slug": task.app_slug,
            "environment": task.environment,
            "reason": task.reason,
        })),
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
    Ok(run_id)
}

/// Tear the sandbox down: its storage silo, its secrets, its Airhouse sibling
/// and ledger rows, its OLTP schema and ledger rows, then its row. `Ok(what was removed)`, or `Err(why it was
/// not)` — and then the row is still there, still `deleting`.
///
/// Waits [`BUSY_RETRY_DELAYS`] for the sandbox's lock; see [`run_with`].
pub async fn run(db: &DatabaseConnection, task: &SandboxTeardownTask) -> Result<String, String> {
    run_with(db, task, &BUSY_RETRY_DELAYS).await
}

/// [`run`], trying the sandbox's lock again after each of `lock_waits`. A
/// sandbox another run — a teardown, or an apply of its migrations — still
/// holds after the last is left alone: `Err`, nothing removed.
pub async fn run_with(
    db: &DatabaseConnection,
    task: &SandboxTeardownTask,
    lock_waits: &[Duration],
) -> Result<String, String> {
    let environment = task.sandbox()?;
    let lock = SandboxLock::acquire(db, task.app_id, &task.environment, lock_waits)
        .await
        .map_err(|e| not_torn_down(task, "take its lock", e.to_string()))?;
    let Some(lock) = lock else {
        let busy = "another teardown or migration of it is still running";
        return Err(not_torn_down(task, "busy", busy.to_string()));
    };
    let outcome = tear_down(db, task, &environment).await;
    lock.release().await;
    outcome
}

fn not_torn_down(task: &SandboxTeardownTask, step: &str, why: String) -> String {
    format!(
        "sandbox {} was not torn down ({step}: {why}); it stays deleting, and deleting it again \
         retries",
        task.environment
    )
}

/// Everything that runs under the sandbox's lock.
async fn tear_down(
    db: &DatabaseConnection,
    task: &SandboxTeardownTask,
    environment: &AppEnvironment,
) -> Result<String, String> {
    let name = &task.environment;
    match standing(db, task)
        .await
        .map_err(|e| not_torn_down(task, "read its row", e.to_string()))?
    {
        Standing::Deleting | Standing::AppGone => {}
        Standing::Active => {
            return Ok(format!(
                "{name} is active: the name was created again since this teardown was queued; \
                 nothing was removed"
            ));
        }
        Standing::TornDown => {
            return Ok(format!(
                "{name} was already torn down by an earlier run; nothing was removed"
            ));
        }
    }
    delete_environment_assets(task.app_id, environment)
        .await
        .map_err(|e| not_torn_down(task, "storage", e.to_string()))?;
    let secrets = delete_environment_secrets(db, task.workspace_id, task.app_id, environment)
        .await
        .map_err(|e| not_torn_down(task, "secrets", e.to_string()))?;
    let dropped = drop_sibling(db, task, environment)
        .await
        .map_err(|e| not_torn_down(task, "Airhouse", e.to_string()))?;
    let oltp = drop_oltp_schema(db, task, environment)
        .await
        .map_err(|e| not_torn_down(task, "OLTP schema", e.to_string()))?;
    let rows = remove_row(db, task)
        .await
        .map_err(|e| not_torn_down(task, "remove its row", e.to_string()))?;
    Ok(format!(
        "{}, {}",
        summary(task, secrets, &dropped, rows),
        oltp_summary(&oltp)
    ))
}

/// The OLTP step: drop the sandbox's own schema on the org's staging branch
/// and its ledger rows — that schema, and no other. The row's state, read
/// here under the lock, is what says there is one; it goes with the row. Any
/// state counts, including one this build cannot read — a newer build's — so
/// a schema is never left behind because its record was not understood.
async fn drop_oltp_schema(
    db: &DatabaseConnection,
    task: &SandboxTeardownTask,
    environment: &AppEnvironment,
) -> Result<SandboxOltpDrop, MigrationError> {
    let recorded = oltp_state::is_recorded(db, task.app_id, environment)
        .await
        .map_err(|e| MigrationError::Db(e.to_string()))?;
    let sandbox = oltp_home::Sandbox {
        app_id: task.app_id,
        app_slug: &task.app_slug,
        org_id: task.org_id,
        environment,
    };
    oltp_home::drop_schema(db, sandbox, recorded).await
}

fn oltp_summary(oltp: &SandboxOltpDrop) -> &'static str {
    match (oltp.attempted, oltp.branch_gone) {
        (false, _) => "no OLTP schema",
        (true, true) => "its OLTP schema went with the org's staging branch",
        (true, false) => "OLTP schema dropped",
    }
}

/// What the sandbox's name stands for when a run takes its lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    /// The row is there and marked: this run's to tear down.
    Deleting,
    /// The row is there and not marked: a sandbox created again.
    Active,
    /// No row, and the app exists: an earlier run finished the job. Whatever
    /// is under the name now belongs to no teardown.
    TornDown,
    /// No row because the app itself was deleted, which removes its rows and
    /// leaves its sandboxes' homes: this run's to remove.
    AppGone,
}

async fn standing(db: &DatabaseConnection, task: &SandboxTeardownTask) -> Result<Standing, DbErr> {
    let row = app_environments::Entity::find_by_id((task.app_id, task.environment.clone()))
        .one(db)
        .await?;
    if let Some(row) = row {
        return Ok(match row.deleting_at {
            Some(_) => Standing::Deleting,
            None => Standing::Active,
        });
    }
    let app: Option<Uuid> = apps::Entity::find_by_id(task.app_id)
        .select_only()
        .column(apps::Column::Id)
        .into_tuple()
        .one(db)
        .await?;
    Ok(match app {
        Some(_) => Standing::TornDown,
        None => Standing::AppGone,
    })
}

/// The Airhouse step, waited out while an apply of the same sandbox holds the
/// target's lock, and dropped if it hangs.
async fn drop_sibling(
    db: &DatabaseConnection,
    task: &SandboxTeardownTask,
    environment: &AppEnvironment,
) -> Result<DropOutcome, MigrationError> {
    let attempt = || async {
        let run = AirhouseDrop {
            app_id: task.app_id,
            app_slug: &task.app_slug,
            workspace_id: task.workspace_id,
        };
        bounded(
            HUNG_APPLY_BACKSTOP,
            drop_environment_schema(db, run, environment),
        )
        .await
        .unwrap_or_else(|| {
            Err(MigrationError::Infra {
                filename: String::new(),
                message: format!("it passed its {}s deadline", HUNG_APPLY_BACKSTOP.as_secs()),
            })
        })
    };
    retry_busy(&BUSY_RETRY_DELAYS, attempt).await
}

/// Remove the row — only one still marked, so a sandbox created again under
/// the name while this ran keeps its row.
async fn remove_row(db: &DatabaseConnection, task: &SandboxTeardownTask) -> Result<u64, DbErr> {
    let deleted = app_environments::Entity::delete_many()
        .filter(app_environments::Column::AppId.eq(task.app_id))
        .filter(app_environments::Column::Name.eq(task.environment.clone()))
        .filter(app_environments::Column::Kind.eq(AppEnvironmentKind::Dev.as_str()))
        .filter(app_environments::Column::DeletingAt.is_not_null())
        .exec(db)
        .await?;
    Ok(deleted.rows_affected)
}

fn summary(task: &SandboxTeardownTask, secrets: usize, dropped: &DropOutcome, rows: u64) -> String {
    let airhouse = match &dropped.schema {
        None => "no Airhouse sibling".to_string(),
        Some(schema) if dropped.schema_dropped => {
            format!(
                "Airhouse sibling {schema} dropped ({} relations)",
                dropped.relations_dropped
            )
        }
        Some(schema) => format!(
            "Airhouse sibling {schema} emptied ({} relations); Airhouse kept the empty schema",
            dropped.relations_dropped
        ),
    };
    let row = if rows == 0 {
        "its row was already gone"
    } else {
        "row removed"
    };
    format!(
        "sandbox {} torn down ({}): storage silo removed, {secrets} secrets deleted, {airhouse}, \
         {row}",
        task.environment, task.reason
    )
}

#[cfg(test)]
#[path = "teardown_tests.rs"]
mod tests;
