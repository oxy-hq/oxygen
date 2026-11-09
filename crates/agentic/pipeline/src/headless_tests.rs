//! The headless eval on a node with no working copy — a serve or worker pod,
//! where Workspace Health runs its agent probe. No database, no LLM: every case
//! here settles at the agent load or at the solver build right after it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{HeadlessError, run_agentic_eval};
use crate::platform::{PlatformContext, ProjectContext};

const AGENT_REF: &str = "_analytics.agentic.yml";

/// A stateless replica: no checkout, and a compile boundary that serves
/// `compiled` for [`AGENT_REF`] and nothing for any other ref.
struct Replica {
    compiled: Option<&'static str>,
}

#[async_trait::async_trait]
impl ProjectContext for Replica {
    async fn resolve_agent_yaml(&self, agent_id: &str) -> Option<String> {
        (agent_id == AGENT_REF)
            .then_some(self.compiled)
            .flatten()
            .map(str::to_string)
    }
    async fn resolve_connector(&self, _db: &str) -> Option<agentic_connector::ConnectorConfig> {
        None
    }
    async fn resolve_model(
        &self,
        _model_ref: Option<&str>,
        _explicit: bool,
    ) -> Option<agentic_analytics::config::ResolvedModelInfo> {
        None
    }
    async fn resolve_secret(&self, _var: &str) -> Option<String> {
        None
    }
}

#[async_trait::async_trait]
impl agentic_automation::WorkspaceContext for Replica {
    fn workspace_path(&self) -> Option<&Path> {
        None
    }
    fn database_configs(&self) -> Vec<oxy_airlayer_compat::DatabaseConfig> {
        vec![]
    }
    async fn get_connector(
        &self,
        name: &str,
    ) -> Result<Arc<dyn agentic_connector::DatabaseConnector>, String> {
        Err(format!("no connector {name}"))
    }
    async fn get_integration(
        &self,
        name: &str,
    ) -> Result<agentic_automation::workspace::IntegrationConfig, String> {
        Err(format!("no integration {name}"))
    }
    async fn list_automation_files(&self) -> Result<Vec<PathBuf>, String> {
        Ok(vec![])
    }
    async fn resolve_automation_yaml(&self, _r: &str) -> Result<String, crate::WorkspaceReadError> {
        Err("unused".into())
    }
}

fn replica(compiled: Option<&'static str>) -> Arc<dyn PlatformContext> {
    Arc::new(Replica { compiled })
}

/// THE case: the agent is in the compiled revision and there is no file to
/// read. The eval must load it from the revision and get as far as building
/// the solver — which then fails for the fake's reason (it resolves no
/// connector), not for a missing file.
#[tokio::test]
async fn an_agent_in_the_compiled_revision_loads_without_a_working_copy() {
    let err = run_agentic_eval(
        replica(Some("databases:\n  - warehouse\n")),
        AGENT_REF,
        "How many orders?".into(),
    )
    .await
    .expect_err("the fake resolves no connector, so the run cannot build");

    let HeadlessError::Run(msg) = &err else {
        panic!("the agent was not loaded from the compiled revision: {err}");
    };
    assert!(msg.contains("no databases configured"), "{msg}");
}

/// The control: an agent the revision does not hold, on a node with nothing to
/// fall back to, still fails at the load — so the case above is not passing
/// because the load stopped being checked.
#[tokio::test]
async fn an_agent_the_revision_does_not_hold_fails_at_the_load() {
    let err = run_agentic_eval(replica(None), AGENT_REF, "q".into())
        .await
        .expect_err("nothing serves this agent");

    assert!(matches!(err, HeadlessError::Load(_)), "got {err:?}");
    assert!(
        err.to_string().contains(AGENT_REF),
        "names the agent: {err}"
    );
}

/// A ref that would walk out of the workspace root on the disk fallback — an
/// absolute path, a `..` — is refused before anything is read, the containment
/// `ConfigManager::resolve_file` gave the probe when it went through it.
#[tokio::test]
async fn a_ref_outside_the_workspace_is_refused_before_the_load() {
    for bad in [
        "../_analytics.agentic.yml",
        "/etc/_analytics.agentic.yml",
        "agents/../../x",
        "",
    ] {
        let err = run_agentic_eval(replica(Some("databases: []\n")), bad, "q".into())
            .await
            .expect_err("refused");
        assert!(matches!(err, HeadlessError::Load(_)), "{bad:?}: {err:?}");
        assert!(
            err.to_string().contains("inside the workspace"),
            "{bad:?}: {err}"
        );
    }
}
