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

pub mod compile_dispatcher;
pub mod metric_tree_exec;
pub mod metric_tree_runner;
pub mod preview_airhouse;
pub mod preview_ctx;
pub mod project_ctx;
pub mod scan_source;

// The leaves went down into `oxy-workspace-runtime` on 2026-11-21, re-exported
// here so their paths did not change.
pub use oxy_workspace_runtime::{
    airhouse_pool, app_airhouse, metric_sink, string_literal, thread_owner,
};
// The SQL execute and its HTTP answer followed on 2026-11-22 (W1b), with the
// connector half of `project_ctx` they build on.
pub use oxy_workspace_runtime::{sql_exec, sql_http};

pub use metric_sink::OxyAnalyticsMetricSink;
pub use metric_tree_runner::{OxyMetricTreeRunner, build_query_executor};
pub use project_ctx::OxyProjectContext;
pub use thread_owner::OxyThreadOwnerLookup;
