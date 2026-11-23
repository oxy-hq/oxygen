//! Oxy-specific wiring for the agentic stack: the bridge between the `oxy::*`
//! platform types and the agentic-pipeline port traits.
//!
//! Every entry is `oxy-workspace-runtime`'s now (the last, `preview_ctx`, on
//! 2026-11-22, W5). What is left here are the re-exports that the CLI and
//! `oxy-app`'s own `tests/` still name at their old paths. Decompose SHM
//! (2026-11-23) deleted the ones nothing named any more (`app_airhouse`,
//! `metric_sink`, `metric_tree_exec`, `metric_tree_runner`, `preview_airhouse`,
//! `preview_ctx`, `scan_source`, `sql_exec`, `sql_http`, `string_literal`,
//! `thread_owner` and the `OxyAnalyticsMetricSink`, `OxyMetricTreeRunner` and
//! `build_query_executor` items): import `oxy_workspace_runtime`.

// The Airhouse pool, down on 2026-11-21; `tests/airhouse` keeps the path.
pub use oxy_workspace_runtime::airhouse_pool;
// The project context and the compile dispatcher it builds,
// `oxy-workspace-runtime`'s since 2026-11-22 (W4).
pub use oxy_workspace_runtime::{compile_dispatcher, project_ctx};

pub use oxy_workspace_runtime::project_ctx::OxyProjectContext;
pub use oxy_workspace_runtime::thread_owner::OxyThreadOwnerLookup;
