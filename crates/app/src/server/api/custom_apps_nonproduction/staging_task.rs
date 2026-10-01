//! The staging migrations a publish queues: staging's sibling Airhouse schema
//! (previews P5b) and the org's OLTP staging branch (P4b), one
//! `TaskSpec::Custom` per store, run on the worker fleet by
//! [`super::staging_task_executor`].
//!
//! Every publish moves staging's pointer, so staging's homes must get the
//! build's tables. That apply can take minutes — a deadline of five by
//! default, and a backstop past it for a file stuck inside one statement — so
//! it is neither the publish's to wait for nor a spawn of the request's for a
//! deploy to kill: the publish queues it as the last thing it does, after its
//! pointers moved and its caches dropped, and answers. A worker that dies
//! mid-apply leaves the task to be claimed again, and the ledger makes the
//! re-run apply only what is missing.
//!
//! **Never a failed publish.** A task that cannot be queued is a warning on
//! the publish response; a task that fails is recorded as its run's failure
//! (and a `warn!`), in the words the publish response used to carry. Staging's
//! writes to that store fail until a later publish migrates it.
//!
//! **Once per (app, build, store).** The run id is derived from the three, so
//! queueing the same build's apply again finds the run it already wrote.

use agentic_core::delegation::TaskSpec;
use agentic_runtime::orchestrator::crud::queue::TaskScope;
use sea_orm::{DatabaseConnection, DbErr, SqlErr, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::server::api::custom_apps_migrations::DeclaredMigration;

/// The `TaskSpec::Custom` kind, and the run's `source_type` — a platform
/// daemon's, so the coordinator feed and workspace health both leave it out.
pub const STAGING_MIGRATIONS_KIND: &str = "custom_app_staging_migrations";

/// Which of staging's homes a task migrates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StagingStore {
    /// The app schema's sibling, `app_<writer>__staging`.
    Airhouse,
    /// The org's OLTP staging branch, when it has one.
    OltpBranch,
}

impl StagingStore {
    fn as_str(self) -> &'static str {
        match self {
            Self::Airhouse => "airhouse",
            Self::OltpBranch => "oltp_branch",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Airhouse => "Airhouse schema",
            Self::OltpBranch => "OLTP staging branch",
        }
    }
}

/// One declared file, as the task carries it. The SQL rides in the payload:
/// the build's files are not kept anywhere a worker could re-read them, and a
/// bundle's migrations are kilobytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedMigration {
    pub filename: String,
    pub checksum: String,
    pub sql: String,
}

/// The build a publish queues staging's migrations for.
#[derive(Clone, Copy, Debug)]
pub struct StagingBuild<'a> {
    pub app_id: Uuid,
    pub app_slug: &'a str,
    /// The app's workspace: where the run is filed, and whose Airhouse the
    /// sibling schema lives in.
    pub workspace_id: Uuid,
    pub org_id: Uuid,
    pub build_pk: Uuid,
}

/// The payload of one staging migration task.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagingMigrationTask {
    pub store: StagingStore,
    pub app_id: Uuid,
    pub app_slug: String,
    pub workspace_id: Uuid,
    pub org_id: Uuid,
    pub build_pk: Uuid,
    pub migrations: Vec<QueuedMigration>,
}

impl StagingMigrationTask {
    pub fn new(
        store: StagingStore,
        build: &StagingBuild<'_>,
        declared: &[DeclaredMigration],
    ) -> Self {
        Self {
            store,
            app_id: build.app_id,
            app_slug: build.app_slug.to_string(),
            workspace_id: build.workspace_id,
            org_id: build.org_id,
            build_pk: build.build_pk,
            migrations: declared
                .iter()
                .map(|m| QueuedMigration {
                    filename: m.filename.clone(),
                    checksum: m.checksum.clone(),
                    sql: m.sql.clone(),
                })
                .collect(),
        }
    }

    /// The run (and task) id: one per (app, build, store).
    pub fn run_id(&self) -> String {
        format!(
            "{STAGING_MIGRATIONS_KIND}:{}:{}:{}",
            self.store.as_str(),
            self.app_id,
            self.build_pk
        )
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
            kind: STAGING_MIGRATIONS_KIND.to_string(),
            payload: serde_json::to_value(self)?,
        })
    }

    /// The payload of a queued spec.
    pub fn from_spec(spec: &TaskSpec) -> Result<Self, String> {
        match spec {
            TaskSpec::Custom { kind, payload } if kind == STAGING_MIGRATIONS_KIND => {
                serde_json::from_value(payload.clone())
                    .map_err(|e| format!("bad staging migration payload: {e}"))
            }
            other => Err(format!("not a staging migration task: {other:?}")),
        }
    }
}

/// Queue staging's migrations for `build`, one task per store that declares
/// files. The warnings are the publish's: only a task that could not be
/// queued has one.
pub async fn queue_staging_migrations(
    db: &DatabaseConnection,
    build: StagingBuild<'_>,
    oltp: &[DeclaredMigration],
    airhouse: &[DeclaredMigration],
) -> Vec<String> {
    let mut warnings = Vec::new();
    for (store, declared) in [
        (StagingStore::Airhouse, airhouse),
        (StagingStore::OltpBranch, oltp),
    ] {
        if declared.is_empty() {
            continue;
        }
        let task = StagingMigrationTask::new(store, &build, declared);
        if let Err(e) = enqueue(db, &task).await {
            tracing::warn!(app_id = %build.app_id, store = store.as_str(), error = %e,
                "publish: could not queue staging's migrations");
            warnings.push(format!(
                "staging's {} migrations were not queued ({e}); the publish went on, and \
                 staging's writes there will fail until a later publish migrates it",
                store.label()
            ));
        }
    }
    warnings
}

/// Queue `task` unless its run already exists. `Ok(true)`: this call queued
/// it; `Ok(false)`: an earlier one had.
pub async fn enqueue(db: &DatabaseConnection, task: &StagingMigrationTask) -> Result<bool, DbErr> {
    let run_id = task.run_id();
    let spec = task.spec().map_err(|e| DbErr::Custom(e.to_string()))?;
    let txn = db.begin().await?;
    let inserted = agentic_runtime::crud::insert_run(
        &txn,
        &run_id,
        &format!(
            "Migrate staging's {} for {}",
            task.store.label(),
            task.app_slug
        ),
        None,
        STAGING_MIGRATIONS_KIND,
        Some(json!({
            "app_id": task.app_id,
            "app_slug": task.app_slug,
            "build_pk": task.build_pk,
            "store": task.store.as_str(),
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
    tracing::info!(app_id = %task.app_id, %run_id, "publish: staging migrations queued");
    Ok(true)
}
