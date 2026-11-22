//! Oxy-specific wiring for the agentic stack.
//!
//! This module is the **only** place in the codebase that bridges between
//! the `oxy::*` platform types and the agentic-pipeline port traits. When
//! the platform refactor lands, most of the churn happens in here.
//!
//! # Contents
//!
//! - [`project_ctx`] — [`OxyProjectContext`] implements
//!   [`agentic_pipeline::platform::ProjectContext`] +
//!   [`agentic_automation::WorkspaceContext`].
//! - [`thread_owner`] — platform threads-table adapter for
//!   [`agentic_pipeline::platform::ThreadOwnerLookup`].
//! - [`preview_airhouse`] — the Airhouse connector a workspace preview run
//!   sends everything through (verified, overlaid, on a scoped credential).

pub mod preview_ctx;

// The leaves went down into `oxy-workspace-runtime` on 2026-11-21, re-exported
// here so their paths did not change.
pub use oxy_workspace_runtime::{
    airhouse_pool, app_airhouse, metric_sink, string_literal, thread_owner,
};
// The SQL execute and its HTTP answer followed on 2026-11-22 (W1b), with the
// connector half of `project_ctx` they build on.
pub use oxy_workspace_runtime::{sql_exec, sql_http};
// The metric-tree runner, its executor-facing core and the semantic scan
// resolution, `oxy-workspace-runtime`'s since 2026-11-22 (W2): re-exported at
// their old paths.
pub use oxy_workspace_runtime::{metric_tree_exec, metric_tree_runner, scan_source};
// The preview connector followed on 2026-11-22 (W3), with its DuckDB double
// (re-exported at `server::previews::airhouse_duckdb`).
pub use oxy_workspace_runtime::preview_airhouse;
// The project context and the compile dispatcher it builds,
// `oxy-workspace-runtime`'s since 2026-11-22 (W4): re-exported at their old
// paths.
pub use oxy_workspace_runtime::{compile_dispatcher, project_ctx};

pub use metric_sink::OxyAnalyticsMetricSink;
pub use metric_tree_runner::{OxyMetricTreeRunner, build_query_executor};
pub use project_ctx::OxyProjectContext;
pub use thread_owner::OxyThreadOwnerLookup;
