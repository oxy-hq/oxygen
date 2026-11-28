//! Which platform tools a solver holds, for a host to check what it attached.

use super::AnalyticsSolver;

impl AnalyticsSolver {
    /// Whether the metric-tree tools (`explain`, `opportunity`,
    /// `sensitivity`, `predict`) are on offer: a runner is attached.
    pub fn has_metric_tree_runner(&self) -> bool {
        self.metric_tree_runner.is_some()
    }

    /// Whether the anomaly tools are on offer: a store is attached.
    pub fn has_anomaly_store(&self) -> bool {
        self.anomaly_store.is_some()
    }

    /// The workspace that scopes every anomaly read and write.
    pub fn workspace_id(&self) -> uuid::Uuid {
        self.workspace_id
    }
}
