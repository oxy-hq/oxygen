use minijinja::Value;
use tracing::Instrument;

use oxy::config::WorkingCopy;
use oxy::{
    adapters::workspace::manager::WorkspaceManager,
    config::constants::EVAL_SOURCE_ROOT,
    exec_runtime::{
        ExecutionContext, ExecutionContextBuilder,
        writer::{BufWriter, EventHandler},
    },
    exec_types::Source,
};
use oxy_shared::errors::OxyError;
use types::{EvalInput, EvalResult};

mod correctness_solver;
mod eval;
mod generator;
mod solver;
mod target_agentic;
pub mod types;

pub struct EvalLauncher {
    execution_context: Option<ExecutionContext>,
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
            execution_context: None,
            buf_writer: BufWriter::new(),
        }
    }

    pub async fn with_workspace(
        mut self,
        workspace: WorkspaceManager<WorkingCopy>,
    ) -> Result<Self, OxyError> {
        self.execution_context = Some(
            ExecutionContextBuilder::new()
                .with_workspace_manager(workspace)
                .with_writer(self.buf_writer.create_writer(None)?)
                .with_global_context(Value::UNDEFINED)
                .with_source(Source {
                    parent_id: None,
                    id: "eval".to_string(),
                    kind: EVAL_SOURCE_ROOT.to_string(),
                })
                .build()?,
        );
        Ok(self)
    }

    pub async fn launch<H: EventHandler + Send + 'static>(
        self,
        eval_input: EvalInput,
        event_handler: H,
    ) -> Result<Vec<Result<EvalResult, OxyError>>, OxyError> {
        let execution_context = self.execution_context.ok_or(OxyError::RuntimeError(
            "ExecutionContext is required".to_string(),
        ))?;

        // Capture the current span to propagate trace context to the spawned task
        let current_span = tracing::Span::current();

        let handle = tokio::spawn(
            async move { eval::run_eval(&execution_context, eval_input).await }
                .instrument(current_span),
        );
        let buf_writer = self.buf_writer;
        let event_handle =
            tokio::spawn(async move { buf_writer.write_to_handler(event_handler).await });
        let response = handle.await?;
        event_handle.await??;
        response
    }
}
