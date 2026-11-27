//! The one queued task that writes a staging attempt's advisory notes: a
//! `TaskSpec::Custom` of kind [`BUILD_CHECKS_AI_KIND`], run on the worker
//! fleet by [`super::executor`].
//!
//! **Once per (app, build, staging attempt).** The run id is derived from the
//! three, so queueing the same attempt again finds the run it already wrote
//! and queues nothing: the settled seam calls once, and a second call would
//! still queue once.

use agentic_core::delegation::TaskSpec;
use agentic_runtime::orchestrator::crud::queue::TaskScope;
use entity::{app_builds, apps};
use sea_orm::{DatabaseConnection, DbErr, SqlErr, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::super::environment::CheckEnvironment;
use super::super::store::AttemptKey;

/// The `TaskSpec::Custom` kind, and the run's `source_type`: a platform
/// daemon's, so the coordinator feed and workspace health leave it out.
pub const BUILD_CHECKS_AI_KIND: &str = "custom_app_build_checks_ai";

/// The payload: one settled staging attempt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildChecksAiTask {
    pub app_id: Uuid,
    pub app_slug: String,
    pub org_id: Uuid,
    /// The app's workspace: where the run is filed, and whose model writes.
    pub workspace_id: Uuid,
    /// The build checked (`app_builds.id`).
    pub build_pk: Uuid,
    /// The staging attempt.
    pub attempt: i32,
}

impl BuildChecksAiTask {
    pub fn of(app: &apps::Model, build: &app_builds::Model, attempt: i32) -> Self {
        Self {
            app_id: app.id,
            app_slug: app.slug.clone(),
            org_id: app.org_id,
            workspace_id: app.project_id,
            build_pk: build.id,
            attempt,
        }
    }

    /// The run (and task) id: one per (app, build, staging attempt).
    pub fn run_id(&self) -> String {
        format!(
            "{BUILD_CHECKS_AI_KIND}:{}:{}:{}",
            self.app_id, self.build_pk, self.attempt
        )
    }

    /// The attempt the notes are about.
    pub fn key(&self) -> AttemptKey {
        AttemptKey {
            build_pk: self.build_pk,
            environment: CheckEnvironment::Staging.name().to_string(),
            attempt: self.attempt,
        }
    }

    pub fn spec(&self) -> Result<TaskSpec, serde_json::Error> {
        Ok(TaskSpec::Custom {
            kind: BUILD_CHECKS_AI_KIND.to_string(),
            payload: serde_json::to_value(self)?,
        })
    }

    /// The payload of a queued spec.
    pub fn from_spec(spec: &TaskSpec) -> Result<Self, String> {
        match spec {
            TaskSpec::Custom { kind, payload } if kind == BUILD_CHECKS_AI_KIND => {
                serde_json::from_value(payload.clone())
                    .map_err(|e| format!("bad build checks notes payload: {e}"))
            }
            other => Err(format!("not a build checks notes task: {other:?}")),
        }
    }
}

/// Queue `task` unless its run already exists. `Ok(true)`: this call queued
/// it; `Ok(false)`: an earlier one had.
pub async fn enqueue(db: &DatabaseConnection, task: &BuildChecksAiTask) -> Result<bool, DbErr> {
    let run_id = task.run_id();
    let spec = task.spec().map_err(|e| DbErr::Custom(e.to_string()))?;
    let txn = db.begin().await?;
    let inserted = agentic_runtime::crud::insert_run(
        &txn,
        &run_id,
        &format!("Notes on the checks of {}", task.app_slug),
        None,
        BUILD_CHECKS_AI_KIND,
        Some(json!({
            "app_id": task.app_id,
            "app_slug": task.app_slug,
            "build_pk": task.build_pk,
            "attempt": task.attempt,
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
    tracing::info!(app_id = %task.app_id, %run_id, "build checks: advisory notes queued");
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task() -> BuildChecksAiTask {
        BuildChecksAiTask {
            app_id: Uuid::from_u128(1),
            app_slug: "store".into(),
            org_id: Uuid::from_u128(2),
            workspace_id: Uuid::from_u128(3),
            build_pk: Uuid::from_u128(4),
            attempt: 2,
        }
    }

    #[test]
    fn one_run_per_staging_attempt_and_the_spec_round_trips() {
        assert_eq!(
            task().run_id(),
            "custom_app_build_checks_ai:00000000-0000-0000-0000-000000000001:\
             00000000-0000-0000-0000-000000000004:2"
        );
        let next = BuildChecksAiTask {
            attempt: 3,
            ..task()
        };
        assert_ne!(task().run_id(), next.run_id());
        assert_eq!(task().key().environment, "staging");
        assert_eq!(task().key().attempt, 2);
        let spec = task().spec().unwrap();
        assert_eq!(BuildChecksAiTask::from_spec(&spec).unwrap(), task());
        // The checks' own task, and what follows a failed production check,
        // are other kinds: neither is taken for this one.
        for other in [
            "custom_app_build_checks",
            "custom_app_production_check_failed",
        ] {
            let spec = TaskSpec::Custom {
                kind: other.into(),
                payload: serde_json::to_value(task()).unwrap(),
            };
            assert!(BuildChecksAiTask::from_spec(&spec).is_err(), "{other}");
        }
    }
}
