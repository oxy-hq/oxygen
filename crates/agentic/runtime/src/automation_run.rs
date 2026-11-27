//! The automation run's vocabulary below the domains: the `source_type` its
//! `agentic_runs` rows carry, the error a run's start and snapshot return,
//! and the public validator of a workflow ref.
//!
//! They moved down from `agentic-automation` (`SOURCE_TYPE`) and
//! `agentic-pipeline` (`automation_run::{AutomationRunError,
//! validate_automation_ref}`) so the workspace-preview queue (`oxy-previews`)
//! can seed a preview automation run without depending on either. Both crates
//! re-export them at their old paths.

use sea_orm::DbErr;

use crate::preview_names;

/// The `source_type` an automation run's `agentic_runs` row carries, and the
/// domain's key in the runtime event registry (the SSE layer looks up the
/// processor for a run's events by it). Re-exported as
/// `agentic_automation::SOURCE_TYPE`.
pub const SOURCE_TYPE: &str = "workflow";

/// Errors from starting, snapshotting or inlining an automation run
/// (`agentic_pipeline::automation_run::{start_automation_run,
/// get_automation_snapshot, run_inline_automation}`).
#[derive(Debug, thiserror::Error)]
pub enum AutomationRunError {
    #[error("database: {0}")]
    Db(#[from] DbErr),
    #[error("run not found")]
    NotFound,
    #[error("inline automation: {0}")]
    Inline(String),
    /// Client-supplied `StartAutomationRequest` failed validation. HTTP
    /// callers should map this to `400 Bad Request` with the message — the
    /// content is shaped for end-user display.
    #[error("invalid request: {0}")]
    InvalidInput(String),
}

/// The public validator of a workflow ref: non-empty, relative to the
/// workspace, no `..` segment, and not carrying the reserved preview prefix.
pub fn validate_automation_ref(workflow_ref: &str) -> Result<(), AutomationRunError> {
    if workflow_ref.is_empty() {
        return Err(AutomationRunError::InvalidInput(
            "workflow_ref is empty".into(),
        ));
    }
    // `preview:<run_id>:<ref>` is what a workspace-preview run's root task
    // carries, and only `seed_preview_automation_run` writes one. From any other
    // caller it would be a forged preview run — or, on a pod that knows
    // previews, a request to be driven like one.
    if preview_names::is_scoped(workflow_ref) {
        return Err(AutomationRunError::InvalidInput(format!(
            "workflow_ref {workflow_ref:?} uses the reserved `{}` prefix",
            preview_names::PREVIEW_PREFIX
        )));
    }
    let candidate = std::path::Path::new(workflow_ref);
    if candidate.is_absolute() {
        return Err(AutomationRunError::InvalidInput(format!(
            "workflow_ref {workflow_ref:?} must be relative to the workspace"
        )));
    }
    if candidate
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(AutomationRunError::InvalidInput(format!(
            "workflow_ref {workflow_ref:?} must not contain `..` segments"
        )));
    }
    Ok(())
}
