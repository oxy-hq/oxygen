//! A user's saved `default_agent_path`, checked against the agents the
//! workspace compiled before a message runs on it.
//!
//! The preference is a stored string, written before classic agents retired
//! and before the lookup was by compile key, so it may name a `.agent.yml`
//! file, a bare stem, or a path. It is resolved through the same door every
//! run uses (`ConfigManager::agent_by_ref`), never by reading the working
//! copy: `/slack/events` is `FleetOk`.
//!
//! - It names a compiled agent: run that agent, by its canonical `file_path`.
//! - It names none (a classic path, a deleted agent): fall back to the
//!   workspace's default agent, or say the workspace has no agents. The same
//!   outcome as no saved agent at all, which is what the planned backfill
//!   produces by nulling such a preference.
//! - The lookup failed: keep the saved ref. The run reads it again and reports
//!   a retryable failure in the thread rather than switching agents silently.

use entity::workspaces;
use oxy::config::{AgentRef, ArtifactError};
use oxy_shared::errors::OxyError;

use super::workspace_agent::{Resolution, resolve_for_workspace};

/// The resolution for workspace `ws` when the user saved `saved` as their
/// default agent.
pub(super) async fn resolve_saved_agent(
    ws: &workspaces::Model,
    saved: &str,
) -> Result<Resolution, OxyError> {
    let found = match crate::integrations::slack::workspace::build_manager(
        ws.id,
        "slack_saved_agent",
    )
    .await
    {
        Ok(wm) => wm.config_manager.agent_by_ref(saved).await,
        Err(e) => Err(ArtifactError::Backend(e.to_string())),
    };
    match decide(saved, found) {
        Saved::Run(agent_path) => Ok(Resolution::Resolved {
            workspace_id: ws.id,
            agent_path,
        }),
        Saved::FallBack => {
            tracing::warn!(
                workspace_id = %ws.id,
                saved,
                "slack: the saved default agent names no compiled agent; using the workspace default"
            );
            resolve_for_workspace(ws).await
        }
    }
}

/// What a saved ref resolves to, apart from the I/O that finds out.
#[derive(Debug, PartialEq, Eq)]
enum Saved {
    /// Run this ref.
    Run(String),
    /// The saved ref names no agent: choose as if none were saved.
    FallBack,
}

fn decide(saved: &str, found: Result<Option<AgentRef>, ArtifactError>) -> Saved {
    match found {
        Ok(Some(agent)) => Saved::Run(agent.file_path),
        Ok(None) => Saved::FallBack,
        Err(error) => {
            tracing::warn!(
                saved,
                error = %error,
                retryable = error.retryable(),
                "slack: could not check the saved default agent; running it as saved"
            );
            Saved::Run(saved.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(file_path: &str) -> AgentRef {
        AgentRef {
            name: file_path.trim_end_matches(".agentic.yml").into(),
            file_path: file_path.into(),
            definition: serde_json::json!({}),
        }
    }

    #[test]
    fn a_saved_ref_that_names_a_compiled_agent_runs_by_its_path() {
        assert_eq!(
            decide("sales", Ok(Some(agent("agents/sales.agentic.yml")))),
            Saved::Run("agents/sales.agentic.yml".into())
        );
    }

    /// The case the retirement left behind: a classic path saved before it.
    #[test]
    fn a_saved_classic_path_falls_back_to_the_workspace_default() {
        assert_eq!(decide("agents/sales.agent.yml", Ok(None)), Saved::FallBack);
    }

    #[test]
    fn a_failed_lookup_keeps_the_saved_ref() {
        let err = ArtifactError::Backend("down".into());
        assert_eq!(
            decide("agents/sales.agentic.yml", Err(err)),
            Saved::Run("agents/sales.agentic.yml".into())
        );
    }
}
