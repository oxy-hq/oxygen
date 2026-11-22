//! The workspace runtime, out of `oxy-app` (`internal-docs/domain-boundaries.md`,
//! "End state: no `oxy-app`"): everything that builds or runs an
//! `OxyProjectContext`, below `oxy-app` and above `oxy-app-core`. No routes,
//! no migrator.
//!
//! It starts with the leaves, which name nothing else of `oxy-app`
//! (2026-11-21). `oxy-app` re-exports each at its old path
//! (`agentic_wiring::{airhouse_pool, app_airhouse, metric_sink,
//! string_literal, thread_owner}`, `server::runtime_artifact`,
//! `server::api::typed_stream`), so the prelude and the tests did not change.
//!
//! - [`airhouse_pool`]: the process-wide Airhouse connection pool, its
//!   credential expiry, the live-connection ceiling and the backpressure retry.
//! - [`app_airhouse`]: `ctx.airhouse`, the app-identity Airhouse connector.
//! - [`metric_sink`]: the analytics metric sink over the observability store.
//! - [`string_literal`]: one SQL string literal per warehouse dialect.
//! - [`thread_owner`]: the threads-table adapter for `ThreadOwnerLookup`.
//! - [`runtime_artifact`]: the S3 read-through mirror for result parquet and
//!   chart PNGs a later request reads on another replica.
//! - [`typed_stream`]: the `TypedRowStream` sinks (Parquet file, JSON rows).
//!
//! Then, on 2026-11-22 (W1b), the connector half of `project_ctx` and what
//! runs on it (`oxy-app` re-exports each at its old path:
//! `agentic_wiring::project_ctx::{build_connector_for_db, …}`,
//! `agentic_wiring::{sql_exec, sql_http}`, `server::preagg_workspace`):
//!
//! - [`connector`]: a configured database in, a connector or its config out
//!   (`*_var` resolution, DuckDB's local and S3-mirror arms, the Airhouse wire
//!   endpoint and credential mint).
//! - [`sql_exec`]: the Dev Portal's SQL execute over that connector.
//! - [`sql_http`]: its HTTP answer (`SqlErrorResponse`, `execute_sql_response`).
//! - [`preagg_workspace`]: the workspace manager the preagg cycle builds.
//!
//! The rest of `agentic_wiring` (the project context, the metric-tree
//! executor, the preview runtime) follows in later PRs.

pub mod airhouse_pool;
pub mod app_airhouse;
pub mod connector;
pub mod metric_sink;
pub mod preagg_workspace;
pub mod runtime_artifact;
pub mod sql_exec;
pub mod sql_http;
pub mod string_literal;
pub mod thread_owner;
pub mod typed_stream;
