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

pub mod airhouse_pool;
pub mod app_airhouse;
pub mod compile_dispatcher;
pub mod metric_sink;
pub mod metric_tree_exec;
pub mod metric_tree_runner;
pub mod preview_airhouse;
pub mod preview_ctx;
pub mod project_ctx;
pub mod scan_source;
pub mod sql_exec;
pub mod sql_http;
pub mod string_literal;
pub mod thread_owner;

pub use metric_sink::OxyAnalyticsMetricSink;
pub use metric_tree_runner::{OxyMetricTreeRunner, build_query_executor};
pub use project_ctx::OxyProjectContext;
pub use thread_owner::OxyThreadOwnerLookup;
