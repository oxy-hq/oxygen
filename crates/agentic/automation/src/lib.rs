pub mod airway_admission;
pub mod completion_policy;
pub mod config;
pub mod error;
pub mod export;
pub mod extension;
pub mod hash;
pub mod preagg_event;
pub mod preview_names;
pub(crate) mod render;
pub mod resolve;
pub mod review;
pub mod runner;
pub mod semantic_bridge;
pub mod step_decider;
pub mod step_executor;
pub mod step_hash;
pub mod step_orchestrator;
pub mod variables;
pub mod workspace;

// Re-export the shared semantic helpers so existing call sites
// (`agentic_automation::semantic::*`, `agentic_automation::preagg::*`,
// `agentic_automation::refresh_key_cache::*`) keep working after the
// extraction into `agentic-semantic`.
pub use agentic_semantic::compile as semantic;
pub use agentic_semantic::preagg;
pub use agentic_semantic::refresh_key_cache;

pub use airway_admission::AirwayAdmissionResolver;
pub use completion_policy::{AutomationCompletionPolicy, AutomationDelegationResolver};
pub use config::{AutomationConfig, TaskType};
pub use error::AutomationError;
pub use extension::AutomationMigrator;
pub use resolve::{build_subrun_steps, resolve_sub_automations};
pub use review::{HttpReview, SqlReview};
pub use runner::OxyAutomationRunner;
pub use step_decider::{AutomationDecider, AutomationDecision};
pub use step_executor::{extract_automation_steps, run_automation_step};
pub use step_orchestrator::AutomationStepOrchestrator;
pub use workspace::{ContextRoot, WorkspaceContext, WorkspaceReadError};

// ── Back-compat aliases (Procedures/Workflows → Automations rename) ───────────
// The canonical types above are now `Automation*`. These aliases keep the old
// `Workflow*` / `OxyProcedureRunner` names resolving for external callers
// (`agentic-pipeline`, `app`) and any other consumer during the deprecation
// window. `extension::WorkflowRunState` has its own alias in that module.
pub use completion_policy::AutomationCompletionPolicy as WorkflowCompletionPolicy;
pub use completion_policy::AutomationDelegationResolver as WorkflowDelegationResolver;
pub use config::AutomationConfig as WorkflowConfig;
pub use error::AutomationError as WorkflowError;
pub use extension::AutomationMigrator as WorkflowMigrator;
pub use runner::OxyAutomationRunner as OxyProcedureRunner;
pub use step_decider::{
    AutomationDecider as WorkflowDecider, AutomationDecision as WorkflowDecision,
};
pub use step_orchestrator::AutomationStepOrchestrator as WorkflowStepOrchestrator;

/// `source_type` to register this domain under in the runtime event registry.
/// Used by the SSE layer to look up the right processor for a run's events.
pub const SOURCE_TYPE: &str = "workflow";

/// Build a [`DomainHandler`] for registering workflow events with the
/// runtime's [`EventRegistry`].
///
/// Automation events (`subrun_started`, `subrun_step_started`,
/// `subrun_step_completed`, `subrun_step_cache_hit`,
/// `subrun_completed`) are emitted directly as `(event_type, payload)`
/// JSON pairs by the decider — no transformation needed. The processor is
/// a passthrough that preserves both fields verbatim.
pub fn event_handler() -> agentic_runtime::event_registry::DomainHandler {
    use agentic_runtime::event_registry::{DomainHandler, RowProcessor};
    use std::sync::Arc;

    let processor: RowProcessor =
        Arc::new(|event_type, payload| Some(vec![(event_type.to_string(), payload.clone())]));

    DomainHandler {
        processor,
        // Automations have no LLM tool/state summaries — the diagram is the UI.
        summary_fn: Arc::new(|_| None),
        tool_summary_fn: Arc::new(|_, _| None),
        // Automation events are independent — no StepEnd metadata accumulation.
        should_accumulate: Some(Arc::new(|_| false)),
    }
}
