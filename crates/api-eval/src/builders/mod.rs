use std::sync::Arc;

use minijinja::Value;
use oxy_app::surface::workspace_runtime::OxyProjectContext;
use tokio::sync::mpsc::Sender;
use tracing::Instrument;

use oxy::config::WorkingCopy;
use oxy::{
    adapters::workspace::manager::WorkspaceManager,
    config::constants::EVAL_SOURCE_ROOT,
    exec_runtime::{
        renderer::Renderer,
        writer::{BufWriter, EventHandler},
    },
    exec_types::{Event, EventKind, ProgressType, Source},
};
use oxy_shared::errors::OxyError;
use types::{EvalInput, EvalResult};

mod answer_judge;
mod correctness_solver;
mod eval;
mod execution_grade;
mod execution_metric;
mod execution_solver;
mod generator;
mod reflect_phase;
mod regrade;
mod run_timing;
mod solver;
mod target_agentic;
pub mod types;

/// The slice of execution state the eval harness actually needs: the workspace
/// (to resolve files and build the agentic judge context), a minijinja renderer
/// (the correctness-prompt template), an event writer (progress /
/// Test-Dashboard SSE / per-run token usage), and a source for event nesting.
///
/// This took the eval harness off the pre-agentic tool-execution hub (since
/// retired). Eval runs both the target agent and the judge on the agentic
/// pipeline (`run_agentic_streaming` / `prepare_one_shot`), so it never needed
/// the old hub's filters / connections / identity / sandbox fields — just the
/// four fields above.
#[derive(Clone)]
pub(super) struct EvalRun {
    pub workspace: WorkspaceManager<WorkingCopy>,
    /// The project context every agent run and judge in this eval shares,
    /// when the caller built one: the server's, from
    /// `workspace_runtime::build_workspace_ctx` (compiled revision, the
    /// caller's identity). `None` builds one per use from `workspace`, as
    /// `oxy test` always has.
    project: Option<Arc<OxyProjectContext>>,
    pub source: Source,
    pub renderer: Renderer,
    writer: Sender<Event>,
}

impl EvalRun {
    pub fn with_child_source(&self, source_id: String, kind: String) -> Self {
        Self {
            workspace: self.workspace.clone(),
            project: self.project.clone(),
            source: Source {
                parent_id: Some(self.source.id.clone()),
                id: source_id,
                kind,
            },
            renderer: self.renderer.clone(),
            writer: self.writer.clone(),
        }
    }

    /// The context a run or judge uses: the shared one, or a fresh one over
    /// the workspace.
    pub fn project_ctx(&self) -> Arc<OxyProjectContext> {
        match &self.project {
            Some(ctx) => ctx.clone(),
            None => Arc::new(OxyProjectContext::new(self.workspace.clone())),
        }
    }

    /// Whether a target ref may be checked against a working copy on this
    /// node: only when the eval reads one (`oxy test`). A shared context reads
    /// the compiled revision, and a fleet node holds no files.
    pub fn reads_working_copy(&self) -> bool {
        self.project.is_none()
    }

    pub async fn write_kind(&self, event: EventKind) -> Result<(), OxyError> {
        self.writer
            .send(Event {
                source: self.source.clone(),
                kind: event,
            })
            .await
            .map_err(|err| OxyError::RuntimeError(format!("Failed to send event:\n{err}")))
    }

    pub async fn write_progress(&self, progress: ProgressType) -> Result<(), OxyError> {
        self.write_kind(EventKind::Progress { progress }).await
    }
}

pub struct EvalLauncher {
    eval_run: Option<EvalRun>,
    buf_writer: BufWriter,
}

impl Default for EvalLauncher {
    fn default() -> Self {
        Self::new()
    }
}

impl EvalLauncher {
    pub fn new() -> Self {
        Self {
            eval_run: None,
            buf_writer: BufWriter::new(),
        }
    }

    pub async fn with_workspace(
        mut self,
        workspace: WorkspaceManager<WorkingCopy>,
    ) -> Result<Self, OxyError> {
        self.eval_run = Some(EvalRun {
            workspace,
            project: None,
            source: Source {
                parent_id: None,
                id: "eval".to_string(),
                kind: EVAL_SOURCE_ROOT.to_string(),
            },
            renderer: Renderer::new(Value::UNDEFINED),
            writer: self.buf_writer.create_writer(None)?,
        });
        Ok(self)
    }

    /// Run against a project context the caller built — the server's, over the
    /// compiled revision. Every agent run and judge shares it.
    pub fn with_project(mut self, project: Arc<OxyProjectContext>) -> Result<Self, OxyError> {
        self.eval_run = Some(EvalRun {
            workspace: project.workspace_manager().clone(),
            project: Some(project),
            source: Source {
                parent_id: None,
                id: "eval".to_string(),
                kind: EVAL_SOURCE_ROOT.to_string(),
            },
            renderer: Renderer::new(Value::UNDEFINED),
            writer: self.buf_writer.create_writer(None)?,
        });
        Ok(self)
    }

    pub async fn launch<H: EventHandler + Send + 'static>(
        self,
        eval_input: EvalInput,
        event_handler: H,
    ) -> Result<Vec<Result<EvalResult, OxyError>>, OxyError> {
        let eval_run = self
            .eval_run
            .ok_or(OxyError::RuntimeError("EvalRun is required".to_string()))?;

        // Capture the current span to propagate trace context to the spawned task
        let current_span = tracing::Span::current();

        let handle = tokio::spawn(
            async move { eval::run_eval(&eval_run, eval_input).await }.instrument(current_span),
        );
        let buf_writer = self.buf_writer;
        let event_handle =
            tokio::spawn(async move { buf_writer.write_to_handler(event_handler).await });
        let response = handle.await?;
        event_handle.await??;
        response
    }
}

#[cfg(test)]
#[path = "results_pin_tests.rs"]
mod results_pin_tests;
