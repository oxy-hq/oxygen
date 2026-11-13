//! The platform's side of the graph cores' execution seam
//! ([`oxy_world_model::graph::GraphWarehouse`]): the connector the platform
//! builds for every Dev Portal query, scoped to the caller.

use std::sync::Arc;

use agentic_connector::DatabaseConnector;
use async_trait::async_trait;
use entity::workspace_members::WorkspaceRole;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::{ConfigManager, DiskSlot, ResolveWorkspaceFile};
use oxy_world_model::graph::{GraphConnection, GraphWarehouse};
use uuid::Uuid;

use oxy_app::surface::world_model_exec::{
    SQLParams, SemanticQueryResponse, build_connector, run_via_agentic_connector,
    run_with_connector,
};

/// A workspace's warehouses as one caller reaches them: the same builder,
/// subject and role, and preview hold as the IDE's query endpoints.
pub struct WorkspaceWarehouse<S: DiskSlot> {
    pub workspace_manager: WorkspaceManager<S>,
    pub user_id: Uuid,
    pub role: WorkspaceRole,
}

#[async_trait]
impl<S> GraphWarehouse for WorkspaceWarehouse<S>
where
    S: DiskSlot + Send + Sync + 'static,
    ConfigManager<S>: ResolveWorkspaceFile,
    WorkspaceManager<S>: Send + Sync,
{
    async fn query(&self, database: &str, sql: String) -> Result<Vec<Vec<String>>, String> {
        let payload = SQLParams {
            sql,
            database: database.to_string(),
            filters: None,
            connections: None,
            result_format: None,
            untyped: false,
        };
        match run_via_agentic_connector(
            &self.workspace_manager,
            self.user_id,
            self.role.clone(),
            &payload,
        )
        .await
        {
            Ok(SemanticQueryResponse::Json(rows)) => Ok(rows),
            // No `result_format` asks for JSON; anything else is not rows.
            Ok(_) => Err("the warehouse answered in a format the graph does not read".into()),
            Err(e) => Err(e.debug_string()),
        }
    }

    async fn connect(&self, database: &str) -> Result<Arc<dyn GraphConnection>, String> {
        build_connector(
            &self.workspace_manager,
            self.user_id,
            self.role.clone(),
            database,
        )
        .await
        .map(|connector| Arc::new(Connection(connector)) as Arc<dyn GraphConnection>)
        .map_err(|e| e.to_string())
    }
}

struct Connection(Arc<dyn DatabaseConnector>);

#[async_trait]
impl GraphConnection for Connection {
    async fn rows(&self, sql: &str) -> Vec<Vec<String>> {
        run_with_connector(&self.0, sql).await
    }
}
