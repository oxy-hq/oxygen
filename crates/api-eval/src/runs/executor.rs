//! `EvalRunExecutor`: runs one queued `eval_run` task on the worker fleet.
//!
//! It builds the workspace's context over the promoted revision
//! (`workspace_runtime::build_workspace_ctx`, the health eval's builder: no
//! working copy, the compiled semantic scan), stamps the caller's identity on
//! it, reads each test file from the compile boundary and runs the same engine
//! `oxy test` runs. The outcome's answer is the results document's text
//! ([`crate::reporters::results_json`]), which the coordinator stores on the
//! run row (`agentic_runs.answer`) for `GET …/evals/runs/{id}`.

use std::sync::Arc;

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use futures::FutureExt;
use oxy::config::EvalTest;
use oxy::config::test_config::TestFileConfig;
use oxy::exec_runtime::writer::EventHandler;
use oxy::exec_types::Event;
use oxy_app::surface::workspace_runtime::{OxyProjectContext, build_workspace_ctx};
use oxy_shared::errors::OxyError;
use sea_orm::DatabaseConnection;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::request::EvalRunPayload;
use crate::builders::EvalLauncher;
use crate::builders::types::{EvalInput, EvalResult, RegradeSource};

/// The `kind` of the queued task. Persisted in `agentic_task_queue`: never
/// rename it.
pub const EVAL_RUN_KIND: &str = "eval_run";

/// The run row's `source_type`, which `GET …/evals/runs/{id}` filters on.
pub const EVAL_RUN_SOURCE: &str = "eval_run";

pub struct EvalRunExecutor {
    pub db: DatabaseConnection,
}

#[async_trait]
impl TaskExecutor for EvalRunExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let TaskSpec::Custom { kind, payload } = &assignment.spec else {
            return Err(format!(
                "unexpected spec for EvalRunExecutor: {:?}",
                assignment.spec
            ));
        };
        if kind != EVAL_RUN_KIND {
            return Err(format!("unknown eval kind: {kind}"));
        }
        let payload: EvalRunPayload = serde_json::from_value(payload.clone())
            .map_err(|e| format!("bad eval_run payload: {e}"))?;

        let (_event_tx, event_rx) = mpsc::channel(4);
        let (outcome_tx, outcome_rx) = mpsc::channel(4);
        let cancel = CancellationToken::new();
        let db = self.db.clone();
        let watch = cancel.clone();
        tokio::spawn(async move {
            let outcome = tokio::select! {
                outcome = guarded(db, payload) => outcome,
                () = watch.cancelled() => TaskOutcome::Cancelled,
            };
            let _ = outcome_tx.send(outcome).await;
        });
        Ok(ExecutingTask {
            events: event_rx,
            outcomes: outcome_rx,
            cancel,
            answers: None,
        })
    }
}

/// A terminal outcome whatever happens: a panic inside the engine would
/// otherwise drop the sender with nothing sent and leave the run `running`.
async fn guarded(db: DatabaseConnection, payload: EvalRunPayload) -> TaskOutcome {
    let workspace_id = payload.workspace_id;
    match std::panic::AssertUnwindSafe(run_queued(&db, payload))
        .catch_unwind()
        .await
    {
        Ok(Ok(answer)) => TaskOutcome::Done {
            answer,
            metadata: None,
        },
        Ok(Err(e)) => TaskOutcome::Failed(e),
        Err(panic) => {
            let msg = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_string());
            tracing::error!(target: "eval_run", %workspace_id, panic = %msg, "eval run panicked");
            TaskOutcome::Failed(format!("eval run panicked: {msg}"))
        }
    }
}

/// Run every test file of the payload, in order, and render the results
/// document `oxy test` wrote for the same files.
async fn run_queued(db: &DatabaseConnection, payload: EvalRunPayload) -> Result<String, String> {
    let ctx = project_for(db, &payload).await?;
    let mut all = Vec::new();
    for file in &payload.test_files {
        all.extend(
            run_one_file(&ctx, &payload, file)
                .await
                .map_err(|e| e.to_string())?,
        );
    }
    crate::reporters::results_json(&all).map_err(|e| e.to_string())
}

/// The workspace's context over the promoted revision, as the caller.
async fn project_for(
    db: &DatabaseConnection,
    payload: &EvalRunPayload,
) -> Result<Arc<OxyProjectContext>, String> {
    let ctx = build_workspace_ctx(payload.workspace_id, db)
        .await
        .ok_or_else(|| format!("workspace {} could not be loaded", payload.workspace_id))?;
    // Freshly built, so this is the only handle; stamping the identity needs
    // the value, and `airhouse_managed` mints the caller's role from it.
    let ctx = Arc::try_unwrap(ctx)
        .map_err(|_| "the workspace context is shared; cannot stamp the caller".to_string())?
        .with_subject(payload.user_id);
    let ctx = match payload.role.clone() {
        Some(role) => ctx.with_role(role),
        None => ctx,
    };
    Ok(Arc::new(ctx))
}

async fn run_one_file(
    ctx: &Arc<OxyProjectContext>,
    payload: &EvalRunPayload,
    file: &str,
) -> Result<Vec<EvalResult>, OxyError> {
    let entry = match ctx
        .workspace_manager()
        .config_manager
        .eval_test(file)
        .await?
    {
        EvalTest::Found(entry) => entry,
        EvalTest::Missing => {
            return Err(OxyError::ConfigurationError(format!(
                "{file} is not in the promoted revision"
            )));
        }
        EvalTest::NotCompiled => {
            return Err(OxyError::ConfigurationError(format!(
                "the promoted revision was compiled before test files were; recompile it, then run {file} again"
            )));
        }
    };
    let test = TestFileConfig::from_definition(&entry.file_path, entry.definition)
        .map_err(OxyError::ConfigurationError)?;
    let input = EvalInput {
        index: payload.case_index,
        target_ref: entry.file_path.clone(),
        tag: payload.tag.clone(),
        regrade_from: payload.regrade.clone().map(RegradeSource::Text),
        test: Some(test),
    };
    let mut results: Vec<EvalResult> = EvalLauncher::new()
        .with_project(ctx.clone())?
        .launch(input, Quiet)
        .await?
        .into_iter()
        .collect::<Result<_, _>>()?;
    // The file name, not the path: what `oxy test` stamped.
    let name = entry
        .file_path
        .rsplit('/')
        .next()
        .unwrap_or(&entry.file_path)
        .to_string();
    for result in &mut results {
        result.test_name = Some(name.clone());
    }
    Ok(results)
}

/// The engine's progress events have no terminal here to draw on.
struct Quiet;

#[async_trait]
impl EventHandler for Quiet {
    async fn handle_event(&mut self, event: Event) -> Result<(), OxyError> {
        tracing::trace!(target: "eval_run", ?event, "eval event");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assignment(spec: TaskSpec) -> TaskAssignment {
        TaskAssignment {
            task_id: "t".into(),
            parent_task_id: None,
            run_id: "r".into(),
            spec,
            policy: None,
            claim_epoch: agentic_core::NO_CLAIM,
        }
    }

    fn executor() -> EvalRunExecutor {
        EvalRunExecutor {
            db: DatabaseConnection::default(),
        }
    }

    async fn refusal(spec: TaskSpec) -> String {
        match executor().execute(assignment(spec)).await {
            Ok(_) => panic!("expected a refusal"),
            Err(e) => e,
        }
    }

    #[tokio::test]
    async fn refuses_another_kind_a_bad_payload_and_a_non_custom_spec() {
        let wrong = refusal(TaskSpec::Custom {
            kind: "health_eval_workspace".into(),
            payload: serde_json::json!({}),
        })
        .await;
        assert!(wrong.contains("health_eval_workspace"), "{wrong}");

        let bad = refusal(TaskSpec::Custom {
            kind: EVAL_RUN_KIND.into(),
            payload: serde_json::json!({ "test_files": [] }),
        })
        .await;
        assert!(bad.contains("payload"), "{bad}");

        let other = refusal(TaskSpec::Agent {
            agent_id: "a".into(),
            question: "q".into(),
            extra: None,
        })
        .await;
        assert!(other.contains("unexpected spec"), "{other}");
    }

    /// The kind string is persisted on queue rows.
    #[test]
    fn the_task_kind_is_pinned() {
        let kinds: Vec<&str> = crate::tasks().iter().map(|t| t.kind).collect();
        assert_eq!(kinds, ["eval_run"]);
    }
}
