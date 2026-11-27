//! Runs a queued build-checks task ([`super::task`]) on the worker fleet.
//! Registered for [`BUILD_CHECKS_KIND`] by `oxy-server`'s `custom_apps_seams`.
//!
//! **The task only starts the runner.** No check runs on the worker: the task
//! dispatches the GitHub Actions workflow ([`super::dispatch`]) and ends. Its
//! outcome is the run's record:
//!
//! - `Done`, the runner was started: the rows stay `queued` until that runner
//!   claims the attempt;
//! - `Done`, saying so, when this deployment is not configured to start one:
//!   the rows stay `queued` and the attempt is marked as one nothing was
//!   started for (`stalled::NotStarted`), so it reads `skipped` instead of
//!   waiting for a runner that will never come, and a person can still run
//!   the checks by hand (`oxyc checks run … --report`);
//! - `Failed` when GitHub refused or could not be reached: every check of the
//!   attempt still open is closed as `error`, saying so, so none reads as
//!   queued forever. A panic ends the same way.
//!
//! An attempt nobody answers is not this task's to watch: readers tell it
//! from its rows ([`super::stalled`]). Nothing retries a task: the console's
//! *Run again* queues the next attempt.

use std::sync::Once;
use std::time::Duration;

use agentic_core::delegation::{TaskAssignment, TaskOutcome};
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use entity::{app_build_checks, app_builds, apps, organizations};
use futures::FutureExt;
use oxy_app_core::config::{AppChecksConfig, BootConfig};
use sea_orm::{DatabaseConnection, EntityTrait};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::dispatch::{self, Dispatched, RunnerInputs};
use super::kind::Outcome;
use super::stalled::{self, NotStarted};
use super::store;
use super::task::{BUILD_CHECKS_KIND, BuildChecksTask};

pub struct BuildChecksExecutor {
    pub db: DatabaseConnection,
}

#[async_trait]
impl TaskExecutor for BuildChecksExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let task = BuildChecksTask::from_spec(&assignment.spec)?;
        let (event_tx, event_rx) = mpsc::channel(1);
        let (outcome_tx, outcome_rx) = mpsc::channel(1);
        let db = self.db.clone();
        tokio::spawn(async move {
            let config = BootConfig::current().app_checks.clone();
            let outcome = run_guarded(&db, &task, &config, &dispatch::BACKOFF).await;
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

/// The task's outcome. A runner that could not be started, and a panic, end
/// in `Failed` with the attempt's open checks closed.
pub async fn run_guarded(
    db: &DatabaseConnection,
    task: &BuildChecksTask,
    config: &AppChecksConfig,
    backoff: &[Duration],
) -> TaskOutcome {
    let ran = std::panic::AssertUnwindSafe(start_runner(db, task, config, backoff))
        .catch_unwind()
        .await;
    let metadata = json!({
        "app_id": task.app_id,
        "build_pk": task.build_pk,
        "environment": task.environment,
        "attempt": task.attempt,
    });
    let why = match ran {
        Ok(Ok(answer)) => {
            return TaskOutcome::Done {
                answer,
                metadata: Some(metadata),
            };
        }
        Ok(Err(why)) => why,
        Err(_) => "the task that starts it panicked".to_string(),
    };
    tracing::warn!(app_id = %task.app_id, build_pk = %task.build_pk, "{BUILD_CHECKS_KIND}: {why}");
    let closing = Outcome::error(format!("the runner could not be started: {why}"));
    if let Err(e) = store::close_open(db, &task.key(), &closing).await {
        tracing::error!(error = %e, "build checks: could not close the attempt's checks");
    }
    TaskOutcome::Failed(why)
}

fn db_err(e: sea_orm::DbErr) -> String {
    format!("the database could not be read ({e})")
}

/// Start the runner of `task`'s attempt. `Ok` is the run's answer; `Err` is
/// why no runner could be started, in words fit for a check's summary.
async fn start_runner(
    db: &DatabaseConnection,
    task: &BuildChecksTask,
    config: &AppChecksConfig,
    backoff: &[Duration],
) -> Result<String, String> {
    let app = apps::Entity::find_by_id(task.app_id)
        .one(db)
        .await
        .map_err(db_err)?;
    let build = app_builds::Entity::find_by_id(task.build_pk)
        .one(db)
        .await
        .map_err(db_err)?;
    let (Some(app), Some(_build)) = (app, build) else {
        // Its rows went with it (`ON DELETE CASCADE`): nothing to check.
        return Ok("the app or the build is gone; no runner started".to_string());
    };
    let rows = attempt_rows(db, task).await?;
    if !rows.iter().any(stalled::is_open) {
        return Ok("every check already has a verdict; no runner started".to_string());
    }
    if let Some(runner) = stalled::runner_of(&rows) {
        // The task was claimed again after its dispatch went out.
        return Ok(format!("{} already runs this attempt", runner.label));
    }
    let inputs = runner_inputs(db, task, app).await?;
    let client = dispatch::client().map_err(|e| e.to_string())?;
    let dispatched = dispatch::dispatch(&client, config, &inputs, backoff).await;
    answer(db, &task.key(), config, dispatched).await
}

/// The rows of `task`'s attempt. An attempt queued by a binary that wrote
/// none gets its rows now.
async fn attempt_rows(
    db: &DatabaseConnection,
    task: &BuildChecksTask,
) -> Result<Vec<app_build_checks::Model>, String> {
    let key = task.key();
    let rows = store::rows_of(db, &key).await.map_err(db_err)?;
    if !rows.is_empty() {
        return Ok(rows);
    }
    store::insert_queued(db, task.app_id, &key, &task.run_id(), &task.kinds())
        .await
        .map_err(db_err)?;
    store::rows_of(db, &key).await.map_err(db_err)
}

/// What the workflow is started with: the app, its org and the attempt.
async fn runner_inputs(
    db: &DatabaseConnection,
    task: &BuildChecksTask,
    app: apps::Model,
) -> Result<RunnerInputs, String> {
    let org = organizations::Entity::find_by_id(app.org_id)
        .one(db)
        .await
        .map_err(db_err)?
        .ok_or_else(|| "the app's org is gone".to_string())?;
    Ok(RunnerInputs {
        app_id: app.id,
        org_slug: org.slug,
        app_slug: app.slug,
        build_pk: task.build_pk,
        attempt: task.attempt,
        environment: task.environment.clone(),
    })
}

/// The run's answer for what the dispatch did. A deployment that starts no
/// runner marks the attempt as one nothing was started for.
async fn answer(
    db: &DatabaseConnection,
    key: &store::AttemptKey,
    config: &AppChecksConfig,
    dispatched: Result<Dispatched, dispatch::DispatchFailed>,
) -> Result<String, String> {
    match dispatched {
        Ok(Dispatched::Started) => Ok(format!(
            "runner started: {} on {}@{}",
            config.github_workflow, config.github_repo, config.github_ref
        )),
        Ok(Dispatched::NotConfigured(why)) => {
            warn_once_not_configured(&why);
            let marker = NotStarted::not_configured(&why, chrono::Utc::now());
            store::mark_not_started(db, key, &marker)
                .await
                .map_err(db_err)?;
            Ok(format!(
                "no runner started ({why}); the checks stay queued until someone runs them"
            ))
        }
        Err(failed) => Err(failed.to_string()),
    }
}

/// Said once per process: every staging move of an unconfigured deployment
/// would otherwise repeat it.
fn warn_once_not_configured(why: &str) {
    static WARNED: Once = Once::new();
    WARNED.call_once(|| {
        tracing::warn!(
            "{BUILD_CHECKS_KIND}: this deployment starts no promote-checks runner ({why}); \
             builds that reach staging keep their checks queued until someone runs them"
        );
    });
}
