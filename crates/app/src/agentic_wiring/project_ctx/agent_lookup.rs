//! An analytics agent's definition, for [`OxyProjectContext`].
//!
//! The host half of `ProjectContext::resolve_agent`. `ConfigManager::agent_by_ref`
//! owns the choice of source and the lookup order (exact path, path plus
//! `.agentic.yml`, then compile key); this says what each outcome means to a
//! run. This host always answers, so the pipeline never falls back to the
//! context root on disk: on a fleet pod that root holds no agents.

use agentic_automation::WorkspaceReadError;
use oxy::config::{AgentRef, ArtifactError};

use super::OxyProjectContext;

impl OxyProjectContext {
    pub(super) async fn agent_yaml(
        &self,
        agent_ref: &str,
    ) -> Result<Option<String>, WorkspaceReadError> {
        let found = self
            .workspace_manager
            .config_manager
            .agent_by_ref(agent_ref)
            .await;
        if let Err(error) = &found {
            tracing::warn!(
                workspace_id = %self.workspace_manager.workspace_id,
                agent_ref,
                error = ?error,
                retryable = error.retryable(),
                "agent definition could not be read"
            );
        }
        answer(agent_ref, found).map(Some)
    }
}

/// Only a genuine miss is `Missing` (the caller's `404`). A read that could
/// not happen is `Unavailable` (`503`), labelled by `retryable()` the same
/// way `context_documents` labels its failures.
fn answer(
    agent_ref: &str,
    found: Result<Option<AgentRef>, ArtifactError>,
) -> Result<String, WorkspaceReadError> {
    match found {
        Ok(Some(agent)) => serde_yaml::to_string(&agent.definition).map_err(|e| {
            WorkspaceReadError::Invalid(format!(
                "agent `{agent_ref}` ({}) could not be serialised: {e}",
                agent.file_path
            ))
        }),
        Ok(None) => Err(WorkspaceReadError::Missing(format!(
            "agent `{agent_ref}` not found in this workspace"
        ))),
        Err(error) if error.retryable() => Err(WorkspaceReadError::Unavailable(format!(
            "agent `{agent_ref}` could not be read: {error}"
        ))),
        Err(error) => Err(WorkspaceReadError::Invalid(format!(
            "agent `{agent_ref}` is not valid: {error}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use oxy_shared::errors::OxyError;
    use serde_json::json;

    use super::*;

    #[test]
    fn a_found_agent_is_its_definition_as_yaml() {
        let agent = AgentRef {
            name: "agents/sales/foo".into(),
            file_path: "agents/sales/foo.agentic.yml".into(),
            definition: json!({ "description": "sales-foo" }),
        };
        let yaml = answer("agents/sales/foo", Ok(Some(agent))).expect("yaml");
        assert!(yaml.contains("sales-foo"), "{yaml}");
    }

    #[test]
    fn only_a_genuine_miss_is_missing() {
        let miss = answer("agents/x", Ok(None)).expect_err("miss");
        assert!(matches!(miss, WorkspaceReadError::Missing(_)), "{miss:?}");
        assert!(miss.to_string().contains("not found"), "{miss}");
    }

    /// G2: Postgres down, no revision row source, a workspace not on this
    /// node: each is a retry, never "agent not found".
    #[test]
    fn a_failed_read_is_unavailable_not_missing() {
        for error in [
            ArtifactError::Backend("connection refused".into()),
            ArtifactError::NoSource,
            ArtifactError::WorkspaceUnavailable("/workspace".into()),
        ] {
            let read = answer("agents/x", Err(error)).expect_err("unavailable");
            assert!(read.is_unavailable(), "{read:?}");
        }
    }

    #[test]
    fn a_broken_file_is_invalid() {
        let error = ArtifactError::Config(OxyError::ConfigurationError("bad yaml".into()));
        let read = answer("agents/x", Err(error)).expect_err("invalid");
        assert!(read.is_invalid(), "{read:?}");
    }
}
