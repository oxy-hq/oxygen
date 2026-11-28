//! Analytics on the react runtime: the workspace snapshot, the tools that
//! read it, and `query`, the one path to a warehouse for SQL the model writes.
//! The snapshot is the leak boundary: no file outside the agent's `context:`
//! globs is reachable. The read-only gate is the write boundary: `query` runs
//! one read statement. With a host's metric-tree runner, the metric-tree tools
//! (`tools::MetricTreeTool`) also query, but only with SQL airlayer generates
//! over the compiled semantic model, as on the FSM. With a host's anomaly store
//! as well, the anomaly tools (`tools::AnomalyTool`) read the same way, and
//! write only to that store (detected anomalies, the explain cache), never to
//! a warehouse.

pub mod citations;
pub mod files;
pub mod prompt;
pub mod read_only;
pub mod render;
pub mod run;
pub mod tools;
pub mod verified;

pub use files::{FileEntry, FileKind, WorkspaceFiles};
pub use read_only::check_read_only;
pub use tools::{
    ChartKind, ChartSpec, ChartTool, MAX_CHART_ROWS, QueryDeps, QueryTool, ReadTool, SearchTool,
};
pub use verified::{VerifiedQueries, VerifiedQuery};

/// The FSM's day-only date hint (`Today is YYYY-MM-DD.`) in `tz`, else UTC,
/// for `EngineConfig::system_date_hint`. One wording for both runtimes.
pub fn date_hint(tz: Option<chrono_tz::Tz>) -> String {
    crate::solver::AnalyticsSolver::current_date_hint(tz)
}

#[cfg(test)]
mod anomaly_tests;
#[cfg(test)]
mod chart_tests;
#[cfg(test)]
mod metric_tree_tests;
#[cfg(test)]
mod query_routing_tests;
#[cfg(test)]
mod query_tests;
#[cfg(test)]
mod rollup_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod verified_tests;
