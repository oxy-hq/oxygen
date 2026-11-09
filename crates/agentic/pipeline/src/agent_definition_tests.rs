//! The working-copy fallback on a node with a checkout (ide/all roles), where
//! nothing is compiled: `agent_id` comes from an HTTP body or a queued run, and
//! must not read a file outside the workspace root.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::load_analytics_agent;
use crate::platform::{PlatformContext, ProjectContext};

const AGENT_YAML: &str = "databases: []\n";

/// A node whose compile boundary has nothing, so every load takes the disk
/// fallback.
struct NothingCompiled;

#[async_trait::async_trait]
impl ProjectContext for NothingCompiled {
    async fn resolve_agent_yaml(&self, _agent_id: &str) -> Option<String> {
        None
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
impl agentic_automation::WorkspaceContext for NothingCompiled {
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

/// `<tmp>/workspace` (the root, holding `agents/inside.agentic.yml`) beside
/// `<tmp>/outside/secret.agentic.yml`, a valid agent the root must not reach.
struct Layout {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    outside: PathBuf,
}

fn layout() -> Layout {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("workspace");
    let outside = tmp.path().join("outside").join("secret.agentic.yml");
    std::fs::create_dir_all(root.join("agents")).unwrap();
    std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
    std::fs::write(root.join("agents/inside.agentic.yml"), AGENT_YAML).unwrap();
    std::fs::write(&outside, AGENT_YAML).unwrap();
    Layout {
        _tmp: tmp,
        root,
        outside,
    }
}

async fn load(agent_id: &str, root: &Path) -> Result<(), String> {
    let platform: Arc<dyn PlatformContext> = Arc::new(NothingCompiled);
    load_analytics_agent(platform.as_ref(), agent_id, root)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// The control: a file under the root loads, with and without its extension,
/// so the refusals below are not passing because the fallback stopped reading.
#[tokio::test]
async fn an_agent_under_the_root_loads_from_the_working_copy() {
    let l = layout();
    for ok in [
        "agents/inside.agentic.yml",
        "agents/inside",
        "./agents/inside",
    ] {
        load(ok, &l.root)
            .await
            .unwrap_or_else(|e| panic!("{ok}: {e}"));
    }
}

/// An absolute path or a `..` that names a real, valid agent outside the root
/// is refused — before the fix, `base_dir.join` read it and the load succeeded.
#[tokio::test]
async fn an_id_that_walks_out_of_the_root_is_refused() {
    let l = layout();
    let absolute = l.outside.to_str().unwrap().to_string();
    for bad in [
        absolute.as_str(),
        "../outside/secret.agentic.yml",
        "../outside/secret",
        "agents/../../outside/secret",
        "",
    ] {
        let err = load(bad, &l.root).await.expect_err(bad);
        assert!(err.contains("inside the workspace"), "{bad:?}: {err}");
    }
}

/// A symlink under the root that points out of it is refused too: the id is a
/// plain relative name, so only the canonical-prefix check catches it.
#[cfg(unix)]
#[tokio::test]
async fn a_symlink_under_the_root_that_points_out_is_refused() {
    let l = layout();
    std::os::unix::fs::symlink(&l.outside, l.root.join("agents/link.agentic.yml")).unwrap();
    std::os::unix::fs::symlink(l.outside.parent().unwrap(), l.root.join("escape")).unwrap();
    for bad in ["agents/link.agentic.yml", "agents/link", "escape/secret"] {
        let err = load(bad, &l.root).await.expect_err(bad);
        assert!(err.contains("inside the workspace"), "{bad:?}: {err}");
    }
}

/// A symlink that stays inside the root still loads.
#[cfg(unix)]
#[tokio::test]
async fn a_symlink_that_stays_under_the_root_loads() {
    let l = layout();
    std::os::unix::fs::symlink(
        l.root.join("agents/inside.agentic.yml"),
        l.root.join("alias.agentic.yml"),
    )
    .unwrap();
    load("alias", &l.root).await.unwrap();
}
