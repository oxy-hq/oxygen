use minijinja::Value;
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

mod correctness_solver;
mod eval;
mod generator;
mod solver;
mod target_agentic;
pub mod types;

/// The slice of execution state the eval harness actually needs: the workspace
/// (to resolve files and build the agentic judge context), a minijinja renderer
/// (the correctness-prompt template), an event writer (progress /
/// Test-Dashboard SSE / per-run token usage), and a source for event nesting.
///
/// This replaces `oxy::exec_runtime::ExecutionContext` here. Eval runs both the
/// target agent and the judge on the agentic pipeline
/// (`run_agentic_streaming` / `prepare_one_shot`), so it never needed the full
/// hub's filters / connections / identity / sandbox fields. Method names mirror
/// `ExecutionContext` so the builder call sites are unchanged — taking eval off
/// the hub without reshaping its event plumbing.
#[derive(Clone)]
pub(super) struct EvalRun {
    pub workspace: WorkspaceManager<WorkingCopy>,
    pub source: Source,
    pub renderer: Renderer,
    writer: Sender<Event>,
}

impl EvalRun {
    pub fn with_child_source(&self, source_id: String, kind: String) -> Self {
        Self {
            workspace: self.workspace.clone(),
            source: Source {
                parent_id: Some(self.source.id.clone()),
                id: source_id,
                kind,
            },
            renderer: self.renderer.clone(),
            writer: self.writer.clone(),
        }
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
