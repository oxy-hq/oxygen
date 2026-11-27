//! The one queued task that checks a build: a `TaskSpec::Custom` of kind
//! [`BUILD_CHECKS_KIND`], run on the worker fleet by [`super::executor`].
//!
//! **Once per (environment, app, build, attempt).** The run id is derived
//! from the four, so queueing the same attempt again finds the run it already
//! wrote and queues nothing; the `queued` rows are written in the same
//! transaction as the task, so a build's list shows its checks the moment the
//! move that queued them commits.

use agentic_core::delegation::TaskSpec;
use agentic_runtime::orchestrator::crud::queue::TaskScope;
use sea_orm::{DatabaseConnection, DbErr, SqlErr, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::environment;
use super::kind::CheckKind;
use super::store::{self, AttemptKey};

/// The `TaskSpec::Custom` kind, and the run's `source_type` — a platform
/// daemon's, so the coordinator feed and workspace health leave it out.
pub const BUILD_CHECKS_KIND: &str = "custom_app_build_checks";

/// The payload of one build's checks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildChecksTask {
    pub app_id: Uuid,
    pub app_slug: String,
    pub org_id: Uuid,
    /// The app's workspace: where the run is filed.
    pub workspace_id: Uuid,
    /// The build checked (`app_builds.id`).
    pub build_pk: Uuid,
    /// `staging` (every build that reaches it), or `production` (the build
    /// production serves after a move, where the promote checks gate is on).
    pub environment: String,
    pub attempt: i32,
}

impl BuildChecksTask {
    /// The run (and task) id: one per (environment, app, build, attempt).
    pub fn run_id(&self) -> String {
        format!(
            "{BUILD_CHECKS_KIND}:{}:{}:{}:{}",
            self.environment, self.app_id, self.build_pk, self.attempt
        )
    }

    /// The kinds this attempt is queued with. Staging: phase 1's four, and
    /// `smoke` and `changes` too where the org's promote checks gate is on
    /// (`settings::enabled_for`), read when the rows are written. Production:
    /// `production_smoke` alone.
    pub fn kinds(&self) -> Vec<CheckKind> {
        environment::kinds_of(&self.environment, self.org_id)
    }

    pub fn key(&self) -> AttemptKey {
        AttemptKey {
            build_pk: self.build_pk,
            environment: self.environment.clone(),
            attempt: self.attempt,
        }
    }

    pub fn spec(&self) -> Result<TaskSpec, serde_json::Error> {
        Ok(TaskSpec::Custom {
            kind: BUILD_CHECKS_KIND.to_string(),
            payload: serde_json::to_value(self)?,
        })
    }

    /// The payload of a queued spec.
    pub fn from_spec(spec: &TaskSpec) -> Result<Self, String> {
        match spec {
            TaskSpec::Custom { kind, payload } if kind == BUILD_CHECKS_KIND => {
                serde_json::from_value(payload.clone())
                    .map_err(|e| format!("bad build checks payload: {e}"))
            }
            other => Err(format!("not a build checks task: {other:?}")),
        }
    }
}

/// Queue `task` and its `queued` rows ([`BuildChecksTask::kinds`]) unless its
/// run already exists. `Ok(true)`: this call queued it; `Ok(false)`: an
/// earlier one had.
pub async fn enqueue(db: &DatabaseConnection, task: &BuildChecksTask) -> Result<bool, DbErr> {
    let run_id = task.run_id();
    let spec = task.spec().map_err(|e| DbErr::Custom(e.to_string()))?;
    let txn = db.begin().await?;
    let inserted = agentic_runtime::crud::insert_run(
        &txn,
        &run_id,
        &format!("Check build of {} on {}", task.app_slug, task.environment),
        None,
        BUILD_CHECKS_KIND,
        Some(json!({
            "app_id": task.app_id,
            "app_slug": task.app_slug,
            "build_pk": task.build_pk,
            "environment": task.environment,
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
    store::insert_queued(&txn, task.app_id, &task.key(), &run_id, &task.kinds()).await?;
    txn.commit().await?;
    tracing::info!(app_id = %task.app_id, %run_id, "build checks queued");
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(attempt: i32) -> BuildChecksTask {
        BuildChecksTask {
            app_id: Uuid::from_u128(1),
            app_slug: "store".into(),
            org_id: Uuid::from_u128(2),
            workspace_id: Uuid::from_u128(3),
            build_pk: Uuid::from_u128(4),
            environment: "staging".into(),
            attempt,
        }
    }

    #[test]
    fn the_run_id_names_environment_app_build_and_attempt() {
        assert_ne!(task(1).run_id(), task(2).run_id());
        assert!(
            task(1)
                .run_id()
                .starts_with("custom_app_build_checks:staging:")
        );
        assert!(task(1).run_id().ends_with(":1"));
    }

    #[test]
    fn a_production_attempt_is_another_run_with_one_kind() {
        let production = BuildChecksTask {
            environment: "production".into(),
            ..task(1)
        };
        assert_ne!(production.run_id(), task(1).run_id());
        assert!(
            production
                .run_id()
                .starts_with("custom_app_build_checks:production:")
        );
        assert_eq!(production.kinds(), [CheckKind::ProductionSmoke]);
        assert_eq!(production.key().environment, "production");
    }

    #[test]
    fn a_spec_round_trips_and_another_kind_is_refused() {
        let spec = task(3).spec().unwrap();
        assert_eq!(BuildChecksTask::from_spec(&spec).unwrap(), task(3));
        let other = TaskSpec::Custom {
            kind: "custom_app_staging_migrations".into(),
            payload: serde_json::to_value(task(3)).unwrap(),
        };
        assert!(BuildChecksTask::from_spec(&other).is_err());
    }
}
