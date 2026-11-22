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
//! The rest of `agentic_wiring` (the project context, the SQL and metric-tree
//! executors, the preview runtime) follows in later PRs.

pub mod airhouse_pool;
pub mod app_airhouse;
pub mod metric_sink;
pub mod runtime_artifact;
pub mod string_literal;
pub mod thread_owner;
pub mod typed_stream;
