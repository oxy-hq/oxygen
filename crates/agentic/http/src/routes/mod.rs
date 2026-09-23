//! Route handlers:
//!   POST   /runs           — create a run, start pipeline in background
//!   GET    /runs/:id/events — SSE stream (live + postgres catch-up)
//!   POST   /runs/:id/answer — deliver user answer to a suspended run

use serde::{Deserialize, Serialize};

use crate::sse;

// ── Request / response types ──────────────────────────────────────────────────

pub use agentic_pipeline::ThinkingMode;

#[derive(Deserialize)]
pub struct CreateRunRequest {
    pub agent_id: String,
    pub question: String,
    pub thread_id: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub thinking_mode: ThinkingMode,
    /// Structured onboarding context — when present, the prompt is built
    /// server-side from the user's selections (tables, warehouse type, model
    /// config) instead of using `question` directly.
    #[serde(default)]
    pub onboarding_context: Option<agentic_pipeline::onboarding::OnboardingContext>,
    /// When true, the solver auto-accepts all `file_change` tool calls
    /// without suspending for human input. Used by the onboarding flow where
    /// file changes are always accepted automatically.
    #[serde(default)]
    pub auto_accept: bool,
}

#[derive(Serialize)]
pub struct CreateRunResponse {
    pub run_id: String,
    pub thread_id: Option<String>,
}

#[derive(Deserialize)]
pub struct AnswerRequest {
    pub answer: String,
}

#[derive(Deserialize)]
pub struct RunIdPath {
    id: String,
}

#[derive(Deserialize)]
pub struct ThreadIdPath {
    thread_id: String,
}

#[derive(Serialize)]
pub struct RunSummary {
    pub run_id: String,
    pub status: String,
    pub agent_id: String,
    pub question: String,
    pub answer: Option<String>,
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_events: Option<Vec<sse::UiEvent>>,
}

pub mod airway;
pub mod automation;
pub mod run;
mod run_scope;
pub mod thread;

pub use airway::{
    airway_backfill_ranges, airway_coverage, airway_resource_cursors, airway_resume,
    backfill_airway, cancel_airway_run, chunked_backfill, create_airway_run,
    discover_source_tables, list_airway_files, list_runs_for_pipeline, reset_airway_cursors,
    reset_airway_schema,
};
pub use automation::{
    cancel_automation_run, create_automation_run, get_automation_file, get_automation_run,
    latest_run_for_thread, list_automation_files, list_runs_for_automation,
};
pub use run::{
    RevertFileChangesRequest, UpdateThinkingModeRequest, answer_run, cancel_run, create_run,
    revert_file_changes, stream_events, update_thinking_mode,
};
pub use thread::{get_run_by_thread, list_runs_by_thread};
