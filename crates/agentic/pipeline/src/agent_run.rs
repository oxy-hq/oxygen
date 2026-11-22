//! Shared start helper for scheduled agent runs.
//!
//! Mirrors [`crate::automation_run::start_automation_run`] /
//! [`crate::airway_run::start_airway_run`]: seeds a fresh `agentic_runs`
//! row + the analytics extension row, then enqueues a
//! [`TaskSpec::Agent`] for the coordinator to drive through
//! [`crate::executor::PipelineTaskExecutor::execute_agent`].
//!
//! Only used by the scheduler today — top-level chat invocations build
//! the pipeline in-process via `PipelineBuilder`. The two paths converge
//! at the `agentic_runs` row, so reads (history dropdown, thread page,
//! SSE stream) treat scheduled and chat runs identically.

use agentic_core::delegation::TaskSpec;
use agentic_runtime::crud;
use sea_orm::{DatabaseConnection, DbErr, TransactionTrait};
use serde::Deserialize;
use thiserror::Error;
use uuid::Uuid;

/// Inputs for [`start_agent_run`].
#[derive(Debug, Clone, Deserialize)]
pub struct StartAgentRequest {
    /// Workspace-relative path or stem of the `.agentic.yml` config to
    /// run. Accepts the same shapes as `PipelineBuilder.analytics()`:
    /// `agents/foo`, `agents/foo.agentic.yml`, or the bare stem `foo`.
    pub agent_id: String,
    /// The question to ask the agent on every fire.
    pub question: String,
    /// Thread to associate this run with. When set, the run row is
    /// linked via `agentic_runs.thread_id` so the thread page can
    /// recover state on reload. Scheduled runs typically omit this.
    #[serde(default)]
    pub thread_id: Option<Uuid>,
    /// Soft FK → `agentic_schedules.id`. Internal-only — only the
    /// scheduler fire path sets this; HTTP/CLI input cannot, so callers
    /// can't spoof which schedule a run "came from".
    #[serde(skip_deserializing, default)]
    pub schedule_id: Option<String>,
    /// How this run was triggered: `"scheduled"`, `"manual"`,
    /// `"backfill"`. Internal-only — stamped onto
    /// `agentic_runs.metadata.trigger`.
    #[serde(skip_deserializing, default)]
    pub trigger: Option<String>,
    /// The cron-scheduled time this run is replaying (UTC). Set by the
    /// backfill path; stamped onto `agentic_runs.metadata.logical_date`.
    #[serde(skip_deserializing, default)]
    pub logical_date: Option<chrono::DateTime<chrono::Utc>>,
    /// Run id this run is a retry of. Stamped onto
    /// `agentic_runs.metadata.retry_of`.
    #[serde(skip_deserializing, default)]
    pub retry_of: Option<String>,
}

/// Errors from [`start_agent_run`].
#[derive(Debug, Error)]
pub enum AgentRunError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("database error: {0}")]
    Db(#[from] DbErr),
}

impl StartAgentRequest {
    fn validate(&self) -> Result<(), AgentRunError> {
        validate_agent_id(&self.agent_id)?;
        if self.question.trim().is_empty() {
            return Err(AgentRunError::InvalidInput(
                "question must not be empty".into(),
            ));
        }
        Ok(())
    }
}

fn validate_agent_id(agent_id: &str) -> Result<(), AgentRunError> {
    if agent_id.trim().is_empty() {
        return Err(AgentRunError::InvalidInput("agent_id is empty".into()));
    }
    let candidate = std::path::Path::new(agent_id);
    if candidate.is_absolute() {
        return Err(AgentRunError::InvalidInput(format!(
            "agent_id {agent_id:?} must be relative to the workspace"
        )));
    }
    if candidate
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(AgentRunError::InvalidInput(format!(
            "agent_id {agent_id:?} must not contain `..` segments"
        )));
    }
    Ok(())
}

/// Insert an `agentic_runs` row + `analytics_run_extensions` row and
/// enqueue a [`TaskSpec::Agent`] for the coordinator to drive.
///
/// Returns the freshly minted `run_id`. `scope` should be
/// [`TaskScope::Global`](crud::TaskScope::Global) for scheduler-driven
/// runs (so the standalone consumer picks them up); the scoped variant
/// exists for symmetry with the automation / airway seed helpers but is
/// not exercised today.
pub async fn start_agent_run(
    db: &DatabaseConnection,
    request: StartAgentRequest,
    scope: crud::TaskScope,
    workspace_id: Uuid,
) -> Result<String, AgentRunError> {
    request.validate()?;

    let run_id = Uuid::new_v4().to_string();
    // Mirror `pipeline::insert_run`'s metadata shape so existing
    // executor / resume code paths (which read `metadata.agent_id`)
    // work transparently for scheduled runs.
    let mut metadata = serde_json::json!({
        "agent_id": request.agent_id,
        "thinking_mode": serde_json::Value::Null,
        "question": request.question,
    });
    crate::scheduler::stamp_trigger_metadata(
        &mut metadata,
        &request.trigger,
        &request.logical_date,
        &request.retry_of,
    );

    // The run row, its extension and its queue row commit together: a run
    // committed alone is "never started" to a booting server's cleanup, which
    // fails it.
    let txn = db.begin().await?;
    if let Some(schedule_id) = request.schedule_id.as_deref() {
        crud::insert_run_with_schedule(
            &txn,
            &run_id,
            &request.question,
            request.thread_id,
            "analytics",
            Some(metadata),
            schedule_id,
            workspace_id,
        )
        .await?;
    } else {
        crud::insert_run(
            &txn,
            &run_id,
            &request.question,
            request.thread_id,
            "analytics",
            Some(metadata),
            workspace_id,
        )
        .await?;
    }

    agentic_analytics::insert_run_meta(&txn, &run_id, &request.agent_id, None).await?;

    let spec = TaskSpec::Agent {
        agent_id: request.agent_id,
        question: request.question,
        extra: None,
    };
    crud::enqueue_task(&txn, &run_id, &run_id, None, &spec, None, scope).await?;
    txn.commit().await?;

    Ok(run_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(agent_id: &str, question: &str) -> StartAgentRequest {
        StartAgentRequest {
            agent_id: agent_id.to_string(),
            question: question.to_string(),
            thread_id: None,
            schedule_id: None,
            trigger: None,
            logical_date: None,
            retry_of: None,
        }
    }

    fn expect_invalid(req: StartAgentRequest, needle: &str) {
        match req.validate() {
            Err(AgentRunError::InvalidInput(msg)) => assert!(
                msg.contains(needle),
                "error {msg:?} did not contain {needle:?}"
            ),
            other => panic!("expected InvalidInput containing {needle:?}, got {other:?}"),
        }
    }

    #[test]
    fn rejects_empty_agent_id() {
        expect_invalid(base("", "Hi"), "empty");
    }

    #[test]
    fn rejects_absolute_agent_id() {
        expect_invalid(base("/etc/passwd", "Hi"), "must be relative");
    }

    #[test]
    fn rejects_parent_dir_agent_id() {
        for r in ["../etc/passwd", "agents/../../../etc", "..", "a/../b"] {
            expect_invalid(base(r, "Hi"), "`..`");
        }
    }

    #[test]
    fn rejects_empty_question() {
        expect_invalid(base("agents/foo", ""), "question must not be empty");
        expect_invalid(base("agents/foo", "   "), "question must not be empty");
    }

    #[test]
    fn accepts_valid_request() {
        base("agents/foo", "What is revenue?")
            .validate()
            .expect("should accept");
        base("foo.agentic.yml", "Hi")
            .validate()
            .expect("should accept");
        base("foo", "Hi").validate().expect("should accept");
    }
}
