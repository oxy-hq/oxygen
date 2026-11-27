//! Runs a queued notes task ([`super::task`]) on the worker fleet. Registered
//! for [`BUILD_CHECKS_AI_KIND`] by `oxy-server`'s `custom_apps_seams`.
//!
//! The work is [`super::run::run`]; this resolves the model it writes with
//! (the default model of the app's own workspace, [`super::model`]) and turns
//! what happened into the run's outcome:
//!
//! - `Done`: the notes were written, or there was nothing to write (the app
//!   or build is gone, the gate is off, an earlier run wrote them), or the
//!   workspace has no model and the two rows say so;
//! - `Failed`: the database could not be read or written, or the task
//!   panicked. A model call that fails is not a failed task: its note says so.
//!
//! Nothing retries the task, and nothing waits on it: a build's checks, its
//! state and its promote are the same with or without its notes.

use agentic_core::delegation::{TaskAssignment, TaskOutcome};
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use futures::FutureExt;
use sea_orm::DatabaseConnection;
use sentry::SentryFutureExt;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::model::{self, NoteModel};
use super::run;
use super::task::{BUILD_CHECKS_AI_KIND, BuildChecksAiTask};

pub struct BuildChecksAiExecutor {
    pub db: DatabaseConnection,
}

#[async_trait]
impl TaskExecutor for BuildChecksAiExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let task = BuildChecksAiTask::from_spec(&assignment.spec)?;
        let (event_tx, event_rx) = mpsc::channel(1);
        let (outcome_tx, outcome_rx) = mpsc::channel(1);
        let db = self.db.clone();
        // The task reads what an app's pages show. A worker has no request
        // hub, so the custom-app tag is bound here: nothing captured while it
        // runs reaches Sentry as the platform's own.
        let hub = oxy_telemetry::sentry_filter::custom_app_hub();
        tokio::spawn(
            async move {
                let _ = outcome_tx
                    .send(run_with_workspace_model(&db, &task).await)
                    .await;
                drop(event_tx);
            }
            .bind_hub(hub),
        );
        Ok(ExecutingTask {
            events: event_rx,
            outcomes: outcome_rx,
            cancel: CancellationToken::new(),
            answers: None,
        })
    }
}

/// [`run_guarded`] with the default model of the app's own workspace.
async fn run_with_workspace_model(
    db: &DatabaseConnection,
    task: &BuildChecksAiTask,
) -> TaskOutcome {
    match model::of_workspace(db, task.workspace_id).await {
        Ok(model) => run_guarded(db, task, Ok(&model)).await,
        Err(why) => run_guarded(db, task, Err(why)).await,
    }
}

/// The task's outcome; a panic ends in `Failed` like any other failure.
pub async fn run_guarded(
    db: &DatabaseConnection,
    task: &BuildChecksAiTask,
    model: Result<&dyn NoteModel, String>,
) -> TaskOutcome {
    let ran = std::panic::AssertUnwindSafe(run::run(db, task, model))
        .catch_unwind()
        .await;
    let metadata = json!({ "app_id": task.app_id, "build_pk": task.build_pk,
        "attempt": task.attempt });
    let why = match ran {
        Ok(Ok(answer)) => {
            return TaskOutcome::Done {
                answer,
                metadata: Some(metadata),
            };
        }
        Ok(Err(why)) => why,
        Err(_) => "the task panicked".to_string(),
    };
    tracing::error!(app_id = %task.app_id, build_pk = %task.build_pk,
        "{BUILD_CHECKS_AI_KIND}: {why}");
    TaskOutcome::Failed(why)
}
