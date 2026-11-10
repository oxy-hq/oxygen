//! External-source reconciliation: compare an Oxy measure against a live
//! external source (Toast first) and fold drift into the health rollup.
pub mod compare;
pub mod config;
pub mod oxy_query;
pub mod segments;
pub mod source;
pub mod toast;
pub mod window;

// Re-exports consumed across this module's siblings (`super::X`) and by the
// evaluator / sweep (`reconcile::X`). Items only ever referenced via their
// submodule path are intentionally not re-exported here.
pub use compare::{
    DriftVerdict, Tolerance, VerdictMeta, compare, error_verdict, unreachable_verdict,
};
pub use config::{Grain, WeekStart, Window};
pub use window::ResolvedWindow;

/// Runs a workspace's `reconcile.yml` checks and returns one verdict each. The
/// eval pass takes it as a trait object so it can run against a fake; the live
/// runner, which builds the workspace's project context, is
/// `oxy-api-workspace-health`'s.
#[async_trait::async_trait]
pub trait ReconcileRunner: Send + Sync {
    async fn run_checks(&self, workspace_id: uuid::Uuid) -> Vec<DriftVerdict>;
}
