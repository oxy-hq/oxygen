//! [`ProjectContext`] for [`PreviewPlatformContext`]. Every method is stated.

use std::sync::Arc;

use agentic_analytics::config::ResolvedModelInfo;
use agentic_connector::{ConnectorConfig, DatabaseConnector};
use agentic_pipeline::SharedMetricSink;
use agentic_pipeline::platform::{
    CompileDispatcher, MonitorScanPort, PreviewScope, ProjectContext, ResolvedPipelineDestination,
};
use async_trait::async_trait;

use super::PreviewPlatformContext;

#[async_trait]
impl ProjectContext for PreviewPlatformContext {
    /// `None`: a bare config is built into an unwrapped connector by the
    /// caller. Everything reaches a database through
    /// [`Self::resolve_pre_built_connector`], which hands out a held one.
    async fn resolve_connector(&self, _db_name: &str) -> Option<ConnectorConfig> {
        None
    }

    /// `None`, except on an Airway sample's platform: airway steps are held
    /// before a destination is resolved, and a destination is a write
    /// credential. A sample lands in the preview's own schemas on a Writer
    /// confined to them (`sample`, `airhouse_samples`).
    async fn resolve_pipeline_destination(
        &self,
        db_name: &str,
        dataset_name: &str,
    ) -> Option<ResolvedPipelineDestination> {
        self.sample_destination(db_name, dataset_name).await
    }

    async fn resolve_pre_built_connector(
        &self,
        db_name: &str,
    ) -> Option<Arc<dyn DatabaseConnector>> {
        match self.held_connector(db_name).await {
            Ok(conn) => Some(conn),
            Err(e) => {
                tracing::warn!(target: "preview", db = %db_name, "preview connector: {e}");
                None
            }
        }
    }

    /// The branch's `config.yml` models, with the workspace's own LLM keys.
    async fn resolve_model(
        &self,
        model_ref: Option<&str>,
        has_explicit_model: bool,
    ) -> Option<ResolvedModelInfo> {
        self.pinned(self.inner.resolve_model(model_ref, has_explicit_model))
            .await
    }

    async fn resolve_agent_yaml(&self, agent_id: &str) -> Option<String> {
        let name = self.own_name(agent_id).ok()?;
        self.pinned(self.inner.resolve_agent_yaml(name)).await
    }

    /// Never production's QuickBooks credentials, and on a rotate-on-use
    /// sample's platform nothing but its registered sandbox's
    /// (`sample_secrets`).
    async fn resolve_secret(&self, var_name: &str) -> Option<String> {
        if self.secret_withheld(var_name) {
            tracing::info!(target: "preview", var = %var_name,
                "a withheld credential does not resolve in a preview");
            return None;
        }
        ProjectContext::resolve_secret(&self.inner, var_name).await
    }

    /// Refused, except the rotated token of a sample's own sandbox grant,
    /// updated in place and never created (`sample_secrets`): the sampler is
    /// that grant's only rotator.
    async fn persist_secret(&self, var_name: &str, value: &str) -> Result<(), String> {
        self.persist_sandbox_token(var_name, value).await
    }

    fn workspace_id(&self) -> uuid::Uuid {
        self.workspace_id
    }

    fn timezone(&self) -> Option<chrono_tz::Tz> {
        self.inner.timezone()
    }

    /// Off: usage metrics would be attributed to the workspace production runs.
    fn metric_sink(&self) -> Option<SharedMetricSink> {
        None
    }

    fn metric_tree_runner(&self) -> Option<Arc<dyn agentic_analytics::MetricTreeRunner>> {
        None
    }

    /// Off: the runner builds its own connectors, outside the held ones.
    fn metric_tree_runner_system(&self) -> Option<Arc<dyn agentic_analytics::MetricTreeRunner>> {
        None
    }

    /// Off: the anomaly tools write anomaly rows.
    fn anomaly_store(&self) -> Option<Arc<dyn agentic_analytics::anomaly_store::AnomalyStore>> {
        None
    }

    fn as_monitor_scan_port(&self) -> Option<&dyn MonitorScanPort> {
        None
    }

    fn compile_dispatcher(&self) -> Option<Arc<dyn CompileDispatcher>> {
        None
    }

    fn preview_scope(&self) -> Option<PreviewScope> {
        Some(self.scope.clone())
    }

    fn is_workspace_preview(&self) -> bool {
        true
    }
}
