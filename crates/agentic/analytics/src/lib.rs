//! Analytics domain implementation.
//!
//! Implements [`Domain`] for structured data analytics queries — turning
//! natural-language questions into resolved query specs that can be executed
//! against a columnar data store.

#[doc(hidden)]
pub mod airlayer_compat;
pub mod anomaly_period;
pub mod anomaly_store;
pub mod brief;
mod catalog;
pub mod config;
pub(crate) mod context_budget;
pub(crate) mod engine;
mod events;
pub mod extension;
mod llm;
pub mod metric_sink;
pub mod metric_tree_runner;
pub mod pipeline;
mod preagg_exec;
pub mod react;
mod schemas;
mod semantic;
mod solver;
pub(crate) mod tools;
mod types;
mod ui;
mod validation;

// ── Extension meta facade ────────────────────────────────────────────────────

pub use extension::{
    AnalyticsMigrator, AnalyticsRunMeta, get_run_meta, get_run_metas, insert_run_meta,
    update_run_spec_hint, update_run_thinking_mode,
};

// ── Catalog (SchemaCatalog, for the integration tests) ─────────────────────────

pub use catalog::SchemaCatalog;

// Additional catalog types for integration tests only.
#[doc(hidden)]
pub use catalog::{Catalog, CatalogError, SemanticLayerError};
#[doc(hidden)]
pub use semantic::SemanticCatalog;

// ── Events ──────────────────────────────────────────────────────────────────

pub use agentic_core::subrun::SubrunStep;
pub use events::{AnalyticsEvent, QuerySource};

// LLM types used only by internal code and tests.
#[doc(hidden)]
pub use llm::LlmClient;

// ── Config ──────────────────────────────────────────────────────────────────

pub use config::{AgentConfig, BuildContext, ConfigError, ResolvedModelInfo};

// ── Subrun ──────────────────────────────────────────────────────────────────

pub use agentic_core::subrun::{SubrunError, SubrunOutput, SubrunRef, SubrunRunner};

// ── Pipeline facade ─────────────────────────────────────────────────────────

pub use brief::{is_brief_agent, start_brief_pipeline};
pub use pipeline::{PipelineParams, resume_pipeline, start_pipeline};

// ── Metric sink port ────────────────────────────────────────────────────────

pub use metric_sink::{AnalyticsMetricSink, SharedMetricSink};

// ── Metric tree runner port ─────────────────────────────────────────────────

pub use metric_tree_runner::{MetricTreeRunner, MetricTreeRunnerError, load_tree};

// ── Solver (needed by pipeline's run_agentic_eval) ──────────────────────────

pub use solver::build_analytics_handlers;
/// The ambient data-coverage line both runtimes append beside the date hint.
pub use solver::freshness_hint::compute_freshness_hint;

// ── Domain types (only externally needed subset) ────────────────────────────

pub use types::{AnalyticsIntent, ConversationTurn, QuestionType, SpecHint};

// Remaining domain types — re-exported for internal modules and integration
// tests.  External consumers (pipeline, http, cli) should NOT depend on these;
// they are not part of the stable public API.
#[doc(hidden)]
pub use types::{
    AnalyticsAnswer, AnalyticsCatalog, AnalyticsDomain, AnalyticsError, AnalyticsResult,
    AnalyticsSolution, ChartConfig, DomainHypothesis, MissingMember, MissingMemberKind,
    QueryRequestItem, QueryResultSet, QuerySpec, ResultShape, SolutionPayload, SolutionSource,
};

// ── UI (crate-internal, used by event_handler below) ────────────────────────

use ui::{analytics_step_summary, analytics_tool_summary};

// ── Event registry ──────────────────────────────────────────────────────────

/// Create a [`DomainHandler`] for registering analytics events with the runtime's
/// [`EventRegistry`].
pub fn event_handler() -> agentic_runtime::event_registry::DomainHandler {
    use agentic_runtime::event_registry::{DomainHandler, domain_row_processor};
    use std::sync::Arc;

    DomainHandler {
        processor: domain_row_processor::<AnalyticsEvent>(),
        summary_fn: Arc::new(analytics_step_summary),
        tool_summary_fn: Arc::new(analytics_tool_summary),
        should_accumulate: Some(Arc::new(|et| {
            matches!(
                et,
                "intent_clarified"
                    | "semantic_shortcut_attempted"
                    | "semantic_shortcut_resolved"
                    | "spec_resolved"
                    | "query_generated"
                    | "query_executed"
            )
        })),
    }
}
