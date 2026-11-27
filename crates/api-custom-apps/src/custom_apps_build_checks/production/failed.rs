//! A production attempt's `production_smoke` failed: the one task that
//! follows it, and who queues it.
//!
//! Queued by the report that **wrote** the `failed` verdict
//! ([`after_report`]): a verdict is written once, so one report does, and the
//! task's run id is derived from (app, build, attempt), so even a second
//! caller would find the run it already wrote. That is what makes "never more
//! than one automatic rollback per production attempt" a property of the
//! queue, before the executor's own guards (`super::rollback`).
//!
//! Only `failed`: an `error` (production never served the build in time, the
//! runner could not judge) and a `skipped` say nothing about what the move
//! broke, so nobody is paged and nothing is rolled back for one.
//!
//! Only where the app's org has the `promote_checks_gate` flag on, read when
//! the verdict is stored: with the flag off nothing new happens.

use agentic_core::delegation::TaskSpec;
use agentic_runtime::orchestrator::crud::queue::TaskScope;
use entity::{app_builds, apps};
use sea_orm::{DatabaseConnection, DbErr, SqlErr, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::super::environment::CheckEnvironment;
use super::super::kind::{CheckKind, CheckStatus, Outcome};
use super::super::settings;
use super::super::store::AttemptKey;

/// The `TaskSpec::Custom` kind, and the run's `source_type`: a platform
/// daemon's, so the coordinator feed and workspace health leave it out.
pub const PRODUCTION_FAILED_KIND: &str = "custom_app_production_check_failed";

/// The payload: one failed production attempt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductionFailedTask {
    pub app_id: Uuid,
    pub app_slug: String,
    pub org_id: Uuid,
    /// The app's workspace: where the run is filed.
    pub workspace_id: Uuid,
    /// The build production served when the check failed (`app_builds.id`).
    pub build_pk: Uuid,
    /// The production attempt.
    pub attempt: i32,
}

impl ProductionFailedTask {
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

    /// The run (and task) id: one per (app, build, production attempt).
    pub fn run_id(&self) -> String {
        format!(
            "{PRODUCTION_FAILED_KIND}:{}:{}:{}",
            self.app_id, self.build_pk, self.attempt
        )
    }

    /// The attempt this follows.
    pub fn key(&self) -> AttemptKey {
        AttemptKey {
            build_pk: self.build_pk,
            environment: CheckEnvironment::Production.name().to_string(),
            attempt: self.attempt,
        }
    }

    pub fn spec(&self) -> Result<TaskSpec, serde_json::Error> {
        Ok(TaskSpec::Custom {
            kind: PRODUCTION_FAILED_KIND.to_string(),
            payload: serde_json::to_value(self)?,
        })
    }

    /// The payload of a queued spec.
    pub fn from_spec(spec: &TaskSpec) -> Result<Self, String> {
        match spec {
            TaskSpec::Custom { kind, payload } if kind == PRODUCTION_FAILED_KIND => {
                serde_json::from_value(payload.clone())
                    .map_err(|e| format!("bad production check payload: {e}"))
            }
            other => Err(format!("not a failed production check task: {other:?}")),
        }
    }
}

/// Queue `task` unless its run already exists. `Ok(true)`: this call queued
/// it; `Ok(false)`: an earlier one had.
pub async fn enqueue(db: &DatabaseConnection, task: &ProductionFailedTask) -> Result<bool, DbErr> {
    let run_id = task.run_id();
    let spec = task.spec().map_err(|e| DbErr::Custom(e.to_string()))?;
    let txn = db.begin().await?;
    let inserted = agentic_runtime::crud::insert_run(
        &txn,
        &run_id,
        &format!("Production check of {} failed", task.app_slug),
        None,
        PRODUCTION_FAILED_KIND,
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
    tracing::info!(app_id = %task.app_id, %run_id, "failed production check: follow-up queued");
    Ok(true)
}

/// Whether a report's writes include a `failed` `production_smoke`: `wrote`
/// is what the report wrote, `verdicts` what it carried.
pub fn wrote_a_failure(wrote: &[CheckKind], verdicts: &[(CheckKind, Outcome)]) -> bool {
    let smoke = CheckKind::ProductionSmoke;
    wrote.contains(&smoke)
        && verdicts
            .iter()
            .any(|(kind, outcome)| *kind == smoke && outcome.status == CheckStatus::Failed)
}

/// After a report's verdicts are stored: queue what follows a failed
/// `production_smoke` (module docs). Never fails the report; a task that
/// could not be queued is logged, and the check's row still says `failed`.
pub async fn after_report(
    db: &DatabaseConnection,
    (app, build): (&apps::Model, &app_builds::Model),
    key: &AttemptKey,
    wrote: &[CheckKind],
    verdicts: &[(CheckKind, Outcome)],
) {
    let production = key.environment == CheckEnvironment::Production.name();
    if !production || !wrote_a_failure(wrote, verdicts) || !settings::enabled_for(app.org_id) {
        return;
    }
    let task = ProductionFailedTask::of(app, build, key.attempt);
    if let Err(e) = enqueue(db, &task).await {
        tracing::error!(app_id = %app.id, build = %build.build_id, attempt = key.attempt,
            error = %e, "failed production check: the follow-up could not be queued");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task() -> ProductionFailedTask {
        ProductionFailedTask {
            app_id: Uuid::from_u128(1),
            app_slug: "store".into(),
            org_id: Uuid::from_u128(2),
            workspace_id: Uuid::from_u128(3),
            build_pk: Uuid::from_u128(4),
            attempt: 2,
        }
    }

    #[test]
    fn one_run_per_production_attempt_and_the_spec_round_trips() {
        assert_eq!(
            task().run_id(),
            "custom_app_production_check_failed:00000000-0000-0000-0000-000000000001:\
             00000000-0000-0000-0000-000000000004:2"
        );
        let next = ProductionFailedTask {
            attempt: 3,
            ..task()
        };
        assert_ne!(task().run_id(), next.run_id());
        assert_eq!(task().key().environment, "production");
        let spec = task().spec().unwrap();
        assert_eq!(ProductionFailedTask::from_spec(&spec).unwrap(), task());
        let other = TaskSpec::Custom {
            kind: "custom_app_build_checks".into(),
            payload: serde_json::to_value(task()).unwrap(),
        };
        assert!(ProductionFailedTask::from_spec(&other).is_err());
    }

    /// Only a `failed` verdict this report wrote: not an `error`, not a pass,
    /// not a failure another report had already written.
    #[test]
    fn only_a_failed_production_smoke_this_report_wrote_is_followed() {
        let smoke = CheckKind::ProductionSmoke;
        let verdict = |status| vec![(smoke, Outcome::new(status, "s", &[]))];
        assert!(wrote_a_failure(&[smoke], &verdict(CheckStatus::Failed)));
        for other in [
            CheckStatus::Error,
            CheckStatus::Passed,
            CheckStatus::Skipped,
        ] {
            assert!(!wrote_a_failure(&[smoke], &verdict(other)), "{other:?}");
        }
        assert!(
            !wrote_a_failure(&[], &verdict(CheckStatus::Failed)),
            "the row already had its verdict"
        );
        let staging = vec![(
            CheckKind::Smoke,
            Outcome::new(CheckStatus::Failed, "s", &[]),
        )];
        assert!(!wrote_a_failure(&[CheckKind::Smoke], &staging));
    }
}
