//! A react run builds no role connection for a database the platform has a
//! read-only connector for; the FSM builds the role connection as before.

use std::path::Path;
use std::sync::{Arc, Mutex};

use agentic_analytics::config::AgentConfig;
use agentic_connector::DatabaseConnector;

use super::{fsm_connectors, react_connectors};
use crate::react_run::tests::{Five, NoBackstop};

/// A host with a read-only connector for one database, as for an
/// `airhouse_managed` one, and a role connection for every database. It
/// records each role connection it is asked to build.
struct Host {
    read_only: &'static str,
    built: Mutex<Vec<String>>,
}

impl Host {
    fn new(read_only: &'static str) -> Self {
        Self {
            read_only,
            built: Mutex::new(Vec::new()),
        }
    }

    fn built(&self) -> Vec<String> {
        self.built.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl crate::platform::ProjectContext for Host {
    async fn resolve_connector(&self, _: &str) -> Option<agentic_connector::ConnectorConfig> {
        None
    }
    async fn resolve_pre_built_connector(&self, name: &str) -> Option<Arc<dyn DatabaseConnector>> {
        self.built.lock().unwrap().push(name.to_string());
        Some(Arc::new(NoBackstop))
    }
    async fn resolve_model(
        &self,
        _: Option<&str>,
        _: bool,
    ) -> Option<agentic_analytics::config::ResolvedModelInfo> {
        None
    }
    async fn resolve_secret(&self, _: &str) -> Option<String> {
        None
    }
    async fn read_only_connector(&self, name: &str) -> Option<Arc<dyn DatabaseConnector>> {
        (name == self.read_only).then(|| Arc::new(Five) as _)
    }
}

#[async_trait::async_trait]
impl agentic_automation::WorkspaceContext for Host {
    fn workspace_path(&self) -> Option<&std::path::Path> {
        None
    }
    fn database_configs(&self) -> Vec<oxy_airlayer_compat::DatabaseConfig> {
        vec![]
    }
    async fn get_connector(&self, name: &str) -> Result<Arc<dyn DatabaseConnector>, String> {
        Err(format!("no connector {name}"))
    }
    async fn get_integration(
        &self,
        name: &str,
    ) -> Result<agentic_automation::workspace::IntegrationConfig, String> {
        Err(format!("no integration {name}"))
    }
    async fn list_automation_files(&self) -> Result<Vec<std::path::PathBuf>, String> {
        Ok(vec![])
    }
    async fn resolve_automation_yaml(&self, _: &str) -> Result<String, crate::WorkspaceReadError> {
        Err("unused".into())
    }
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|n| n.to_string()).collect()
}

fn agent() -> AgentConfig {
    AgentConfig::from_yaml("instructions: hi\ndatabases: [wh, pg]\n").unwrap()
}

#[tokio::test]
async fn react_builds_no_role_connection_where_a_read_only_one_exists() {
    let host = Host::new("wh");
    let (databases, connectors) = react_connectors(&host, &agent(), Path::new(".")).await;
    assert_eq!(databases, names(&["wh", "pg"]));
    assert_eq!(
        host.built(),
        names(&["pg"]),
        "only pg has no read-only path"
    );
    assert!(connectors["wh"].supports_read_only());
    assert!(!connectors["pg"].supports_read_only(), "pg keeps its own");
}

#[tokio::test]
async fn the_fsm_builds_the_role_connection_for_every_database() {
    let host = Host::new("wh");
    let (databases, connectors) = fsm_connectors(&host, &agent(), Path::new(".")).await;
    assert_eq!(databases, names(&["wh", "pg"]));
    assert_eq!(host.built(), names(&["wh", "pg"]));
    assert!(
        !connectors["wh"].supports_read_only(),
        "the role connection"
    );
}
