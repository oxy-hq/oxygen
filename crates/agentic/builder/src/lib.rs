//! Built-in builder copilot agent.
//!
//! A single-state LLM tool loop that lets an LLM read and propose changes to
//! project files, using the same streaming/HITL infrastructure as `agentic-http`.

// SeaORM 2.0's query types nest deeper generically than 1.1's. This crate
// has no sea-orm dependency of its own, but its async fns transitively await
// futures that hold them, and laying those out now exceeds rustc's default
// query depth. Raising the limit is the fix rustc itself suggests.
#![recursion_limit = "256"]

pub mod database;
pub mod events;
pub mod onboarding;
pub mod pipeline;
pub(crate) mod prompts;
pub mod schema_provider;
pub mod secrets;
pub mod semantic;
pub mod solver;
pub mod test_runner;
pub mod tools;
pub mod types;
pub mod ui;
pub mod validator;

pub use database::BuilderDatabaseProvider;
pub use events::BuilderEvent;
pub use pipeline::{BuilderPipelineParams, resume_pipeline, start_pipeline};
pub use prompts::KnowledgeCard;
pub use schema_provider::BuilderSchemaProvider;
pub use secrets::BuilderSecretsProvider;
pub use semantic::{BuilderSemanticCompiler, PreaggHandle, SemanticCompilationResult};
pub use solver::{BuilderSolver, build_builder_handlers};
pub use test_runner::BuilderTestRunner;
pub use types::{
    BuilderAnswer, BuilderDomain, BuilderError, BuilderIntent, BuilderResult, BuilderSolution,
    BuilderSpec, ConversationTurn, ToolExchange,
};
pub use ui::{builder_step_summary, builder_tool_summary};
pub use validator::{BuilderProjectValidator, ValidatedFile, ValidationReport};

// ── Event registry ──────────────────────────────────────────────────────────

/// Create a [`DomainHandler`] for registering builder events with the runtime's
/// [`EventRegistry`].
pub fn event_handler() -> agentic_runtime::event_registry::DomainHandler {
    use agentic_runtime::event_registry::{DomainHandler, domain_row_processor};
    use std::sync::Arc;

    DomainHandler {
        processor: domain_row_processor::<BuilderEvent>(),
        summary_fn: Arc::new(builder_step_summary),
        tool_summary_fn: Arc::new(builder_tool_summary),
        // Builder events (file_changed, tool_used) are streamed as
        // standalone events; they don't need StepEnd metadata enrichment.
        should_accumulate: Some(Arc::new(|_| false)),
    }
}
