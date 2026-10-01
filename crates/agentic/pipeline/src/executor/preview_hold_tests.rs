//! A platform for the airway-hold test: it reports a preview scope (or not),
//! and serves one valid pipeline while counting how often it is asked.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use sea_orm::DatabaseConnection;

use crate::executor::PipelineTaskExecutor;
use crate::platform::{AirwayStepMode, PreviewScope};

pub(crate) const ROOT: &str = "3f0b6c1e-8d2a-4c61-9d7e-0a1b2c3d4e5f";
pub(crate) const PIPELINE: &str = "airway/orders.airway.yml";

const YAML: &str = "\
name: orders
source:
  kind: rest_api
  config:
    base_url: https://example.test
    endpoints:
      - name: orders
        path: /orders
destination:
  database: airhouse
  dataset_name: raw_orders
";

pub(crate) struct PreviewAirwayPlatform {
    pub scope: Option<PreviewScope>,
    /// A request pinned to a preview: a preview with no dry-run scope.
    pub request_preview: bool,
    pub yaml_reads: AtomicUsize,
}

pub(crate) fn preview_scope() -> PreviewScope {
    PreviewScope {
        run_id: ROOT.into(),
        preview_key: "feat_x_abc123".into(),
        revision_id: uuid::Uuid::nil(),
        airway_steps: AirwayStepMode::Hold,
    }
}

/// An executor over `platform` whose database is `Disconnected`: the lease is
/// the first thing past the YAML that touches it, so reaching the lease fails
/// the task instead of passing quietly.
pub(crate) fn executor(platform: Arc<PreviewAirwayPlatform>) -> PipelineTaskExecutor {
    PipelineTaskExecutor::bare(platform, DatabaseConnection::default())
}

#[async_trait]
impl crate::platform::ProjectContext for PreviewAirwayPlatform {
    async fn resolve_connector(&self, _db: &str) -> Option<agentic_connector::ConnectorConfig> {
        None
    }
    async fn resolve_model(
        &self,
        _model_ref: Option<&str>,
        _has_explicit_model: bool,
    ) -> Option<agentic_analytics::config::ResolvedModelInfo> {
        None
    }
    async fn resolve_secret(&self, _var_name: &str) -> Option<String> {
        None
    }
    fn preview_scope(&self) -> Option<PreviewScope> {
        self.scope.clone()
    }
    fn is_workspace_preview(&self) -> bool {
        self.request_preview || self.scope.is_some()
    }
}

#[async_trait]
impl agentic_automation::WorkspaceContext for PreviewAirwayPlatform {
    fn workspace_path(&self) -> Option<&std::path::Path> {
        None
    }
    fn database_configs(&self) -> Vec<oxy_airlayer_compat::DatabaseConfig> {
        vec![]
    }
    async fn get_connector(
        &self,
        _name: &str,
    ) -> Result<Arc<dyn agentic_connector::DatabaseConnector>, String> {
        Err("unused".into())
    }
    async fn get_integration(
        &self,
        _name: &str,
    ) -> Result<agentic_automation::workspace::IntegrationConfig, String> {
        Err("unused".into())
    }
    async fn list_automation_files(&self) -> Result<Vec<std::path::PathBuf>, String> {
        Ok(vec![])
    }
    async fn resolve_automation_yaml(&self, _r: &str) -> Result<String, crate::WorkspaceReadError> {
        Err("unused".into())
    }
    async fn resolve_pipeline_yaml(&self, _r: &str) -> Result<Option<String>, String> {
        self.yaml_reads.fetch_add(1, Ordering::SeqCst);
        Ok(Some(YAML.to_string()))
    }
}
