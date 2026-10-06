//! [`MonitorScanPort`] implementation for [`OxyProjectContext`].

use agentic_automation::WorkspaceContext;
use agentic_pipeline::platform::{MonitorScanPort, ProjectContext};
use async_trait::async_trait;

use super::OxyProjectContext;

#[async_trait]
impl MonitorScanPort for OxyProjectContext {
    async fn run_monitor_scan(
        &self,
        db: &sea_orm::DatabaseConnection,
        workspace_id: uuid::Uuid,
        granularity: &str,
    ) -> Result<String, String> {
        use oxy_metric_monitoring::Granularity;

        let runner = self
            .metric_tree_runner_system()
            .ok_or_else(|| "no metric tree runner available".to_string())?;
        let gran = match granularity {
            "day" => Granularity::Day,
            "week" => Granularity::Week,
            "month" => Granularity::Month,
            other => return Err(format!("unknown granularity: {other:?}")),
        };
        // `.monitor.yml` off the working copy. The compiled arm is
        // `ConfigManager::monitor_config` + `scan::materialise_monitor_config`,
        // which the HTTP `/scan` handler already uses; this system path has not
        // moved yet, so it says what it cannot do instead of resolving against
        // a directory that is not on this node.
        let workspace_root = self.workspace_path().ok_or_else(|| {
            "monitor scan: this node holds no workspace files, so `.monitor.yml` \
             cannot be read"
                .to_string()
        })?;
        let config_path = oxy_metric_monitoring::default_config_path(workspace_root);
        let open_events = oxy_metric_monitoring::load_open_events(db, workspace_id)
            .await
            .map_err(|e| e.to_string())?;
        let scan = oxy_metric_monitoring::scan_workspace(
            runner,
            &config_path,
            chrono::Utc::now(),
            Some(gran),
            &open_events,
        )
        .await
        .map_err(|e| e.to_string())?;
        let persisted = oxy_metric_monitoring::persist_scan(db, workspace_id, &scan)
            .await
            .map_err(|e| e.to_string())?;
        crate::server::anomaly_notify::enqueue_if_due(db, workspace_id, scan.notify.as_ref()).await;
        let summary = format!(
            "scanned={} failed={} persisted={}",
            scan.outcomes.len(),
            scan.failures.len(),
            persisted
        );
        if scan.failures.is_empty() {
            Ok(summary)
        } else {
            Err(summary)
        }
    }
}
