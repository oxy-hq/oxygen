//! The gates that run with no database: they must not reach one.

use std::sync::Arc;

use agentic_connector::DatabaseConnector;
use sea_orm::DatabaseConnection;

use super::{served_memory, writes_held};
use crate::platform::Requester;

/// A host with the `react_memory` flag at `flag` and `requester` as the user.
struct Host {
    flag: bool,
    requester: Option<Requester>,
    staging_app: Option<uuid::Uuid>,
    preview: bool,
}

impl Host {
    fn new(flag: bool, requester: Option<Requester>) -> Self {
        Self {
            flag,
            requester,
            staging_app: None,
            preview: false,
        }
    }
}

#[async_trait::async_trait]
impl crate::platform::ProjectContext for Host {
    async fn resolve_connector(&self, _: &str) -> Option<agentic_connector::ConnectorConfig> {
        None
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
    async fn react_memory_enabled(&self) -> bool {
        self.flag
    }
    fn requester(&self) -> Option<Requester> {
        self.requester.clone()
    }
    fn staging_app_id(&self) -> Option<uuid::Uuid> {
        self.staging_app
    }
    fn is_workspace_preview(&self) -> bool {
        self.preview
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

fn user() -> Option<Requester> {
    Some(Requester {
        user_id: uuid::Uuid::new_v4(),
        role: Some("member".into()),
    })
}

#[tokio::test]
async fn no_memory_with_the_flag_off_or_no_requesting_user() {
    // A disconnected database: either gate reaching it would be a bug.
    let db = DatabaseConnection::default();
    for host in [Host::new(false, user()), Host::new(true, None)] {
        let memory = served_memory(&host, &db, uuid::Uuid::new_v4()).await;
        assert!(memory.notes.is_empty());
        assert!(memory.writes.is_none());
    }
}

#[tokio::test]
async fn unreadable_notes_give_a_run_with_no_memory() {
    let host = Host::new(true, user());
    let memory = served_memory(&host, &DatabaseConnection::default(), uuid::Uuid::new_v4()).await;
    assert!(memory.notes.is_empty());
    assert!(
        memory.writes.is_none(),
        "a run that cannot read its notes writes none either"
    );
}

/// A custom app's staging or sandbox ask, and a workspace preview, hold
/// every write: their runs read notes but save none. Anything else saves.
#[test]
fn staging_sandbox_and_preview_runs_save_no_notes() {
    let staging = Host {
        staging_app: Some(uuid::Uuid::new_v4()),
        ..Host::new(true, user())
    };
    let preview = Host {
        preview: true,
        ..Host::new(true, user())
    };
    assert!(writes_held(&staging));
    assert!(writes_held(&preview));
    assert!(!writes_held(&Host::new(true, user())));
}
