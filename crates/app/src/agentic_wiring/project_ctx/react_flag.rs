//! The per-org `react_runtime` flag, read for this context's workspace.

use sea_orm::EntityTrait;
use uuid::Uuid;

use oxy_feature_flags::{FlagContext, REACT_RUNTIME, is_enabled_for};

use super::OxyProjectContext;

impl OxyProjectContext {
    /// Whether `react_runtime` is on for the workspace's org. Fails closed: a
    /// context with no database, a workspace with no org, or a failed lookup
    /// reads `false`, with a warning, and the run stays on the FSM.
    pub(super) async fn react_runtime_flag(&self) -> bool {
        let workspace_id = self.workspace_manager.workspace_id;
        match self.workspace_org(workspace_id).await {
            Ok(org_id) => is_enabled_for(REACT_RUNTIME, &FlagContext::org(org_id)),
            Err(reason) => {
                tracing::warn!(
                    %workspace_id,
                    reason,
                    "react_runtime: no org to read the flag for; the run stays on the fsm"
                );
                false
            }
        }
    }

    async fn workspace_org(&self, workspace_id: Uuid) -> Result<Uuid, String> {
        let db = self.db.as_deref().ok_or("the context has no database")?;
        let row = entity::workspaces::Entity::find_by_id(workspace_id)
            .one(db)
            .await
            .map_err(|e| format!("workspace lookup failed: {e}"))?
            .ok_or("the workspace row is missing")?;
        row.org_id
            .ok_or_else(|| "the workspace has no org".to_string())
    }
}

#[cfg(test)]
mod tests {
    use agentic_pipeline::platform::ProjectContext;
    use oxy::adapters::workspace::builder::WorkspaceBuilder;
    use oxy_test_utils::fixtures::TestFixture;

    use super::OxyProjectContext;

    /// A context that cannot find its org (here: no database) fails closed.
    #[tokio::test]
    async fn react_runtime_is_off_without_an_org() {
        let fixture = TestFixture::new().expect("tempdir");
        fixture
            .create_file("config.yml", "models: []\ndatabases: []\n")
            .expect("write config.yml");
        let wm = WorkspaceBuilder::new(uuid::Uuid::new_v4())
            .with_working_copy(fixture.path(), None, oxy::config::OnMissing::Fail)
            .await
            .expect("config builder")
            .build()
            .await
            .expect("workspace manager");
        assert!(!OxyProjectContext::new(wm).react_runtime_enabled().await);
    }
}
