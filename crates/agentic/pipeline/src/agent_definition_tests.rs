//! The working-copy fallback on a node with a checkout (ide/all roles), where
//! nothing is compiled: `agent_id` comes from an HTTP body or a queued run, and
//! must not read a file outside the workspace root. And what a host with a
//! compile boundary says is final: a miss is "not found", a failed read is a
//! retry, and neither falls back to the file on this node's disk.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::load_analytics_agent;
use crate::platform::{PlatformContext, ProjectContext};

const AGENT_YAML: &str = "databases: []\n";

type Answer = fn() -> Result<Option<String>, crate::WorkspaceReadError>;

/// A host that says `answer` for every ref. [`NOTHING_COMPILED`] is a host
/// with no compile boundary, so every load takes the disk fallback.
struct Host {
    answer: Answer,
}

const NOTHING_COMPILED: Host = Host {
    answer: || Ok(None),
};

#[async_trait::async_trait]
impl ProjectContext for Host {
    async fn resolve_agent(
        &self,
        _agent_id: &str,
    ) -> Result<Option<String>, crate::WorkspaceReadError> {
        (self.answer)()
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
impl agentic_automation::WorkspaceContext for Host {
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
    load_from(NOTHING_COMPILED, agent_id, root)
        .await
        .map_err(|e| e.to_string())
}

async fn load_from(host: Host, agent_id: &str, root: &Path) -> Result<(), crate::PipelineError> {
    let platform: Arc<dyn PlatformContext> = Arc::new(host);
    load_analytics_agent(platform.as_ref(), agent_id, root)
        .await
        .map(|_| ())
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

/// G2: a read the host could not make is a retry (`503`), not "agent not
/// found", and the valid file in this node's working copy does not answer in
/// its place (G3).
#[tokio::test]
async fn a_failed_host_read_is_retryable_and_never_reads_the_disk() {
    let l = layout();
    let host = Host {
        answer: || Err(crate::WorkspaceReadError::Unavailable("db down".into())),
    };
    let err = load_from(host, "agents/inside", &l.root)
        .await
        .expect_err("the host's answer is final");
    assert!(err.is_retryable(), "{err}");
    assert!(!err.to_string().contains("not found"), "{err}");
}

/// A miss is the host's answer: a permanent "not found", even with the file
/// sitting in this node's working copy.
#[tokio::test]
async fn a_host_miss_is_not_found_and_never_reads_the_disk() {
    let l = layout();
    let host = Host {
        answer: || {
            Err(crate::WorkspaceReadError::Missing(
                "agent `agents/inside` not found".into(),
            ))
        },
    };
    let err = load_from(host, "agents/inside", &l.root)
        .await
        .expect_err("the host's answer is final");
    assert!(!err.is_retryable(), "{err}");
    assert!(err.to_string().contains("not found"), "{err}");
}

/// The path-traversal refusal (#4018) still comes first: a host that would
/// answer is not even asked about an id outside the workspace.
#[tokio::test]
async fn the_workspace_refusal_runs_before_the_host_is_asked() {
    let l = layout();
    let host = Host {
        answer: || Ok(Some(AGENT_YAML.into())),
    };
    let err = load_from(host, "../outside/secret", &l.root)
        .await
        .expect_err("refused");
    assert!(err.to_string().contains("inside the workspace"), "{err}");
}

#[tokio::test]
async fn an_unparseable_host_definition_is_a_permanent_error() {
    let l = layout();
    let host = Host {
        answer: || Ok(Some("databases: {not: [a list".into())),
    };
    let err = load_from(host, "agents/inside", &l.root)
        .await
        .expect_err("invalid");
    assert!(!err.is_retryable(), "{err}");
}
