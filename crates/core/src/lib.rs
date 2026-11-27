#![recursion_limit = "256"]

pub mod adapters;
pub mod api_types;
pub mod config;
pub mod connector;
pub mod constants;
pub mod database;
pub mod exec_runtime;
pub mod exec_types;
/// Moved to `oxy_observability::execution_analytics` (depth stage C, D6,
/// 2026-11-27); re-exported here for one release.
pub use oxy_observability::execution_analytics;
pub mod file_duckdb_limits;
pub mod github;
pub mod metrics;
pub mod observability;
pub mod render;
pub mod semantic;
pub mod service;
pub mod state_dir;
/// `StyledText`, the CLI output styling, lives in `oxy-shared` so a crate can
/// style output without depending on `oxy`; re-exported here for its callers.
pub use oxy_shared::theme;
pub mod types;
pub mod utils;
pub mod workspace_fs_probe;
pub use oxy_shared as shared;
pub use render::{ClientRenderer, render_stream};
