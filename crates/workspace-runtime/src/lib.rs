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
//! Then, on 2026-11-22 (W2), the metric tree's runtime and the semantic scan
//! resolution (`oxy-app` re-exports each at its old path:
//! `agentic_wiring::{metric_tree_runner, metric_tree_exec, scan_source}`):
//!
//! - [`metric_tree_runner`]: `OxyMetricTreeRunner`, the `MetricTreeRunner`
//!   over the semantic engine and the connector pool, and the query executors.
//! - [`metric_tree_exec`]: the executor-facing core the metric-tree surfaces
//!   share (the error, scan and layer resolution, the projection core).
//! - [`scan_source`]: where a semantic read finds the model (the compiled
//!   revision first, the working copy second, a compile enqueued on a miss).
//!
//! Then, on 2026-11-22 (W3), the preview connector and its DuckDB double
//! (`oxy-app` re-exported each at its old path, `agentic_wiring::preview_airhouse`
//! and `server::previews::airhouse_duckdb`, until decompose SHM):
//!
//! - [`preview_airhouse`]: the Airhouse connector a workspace preview run
//!   sends everything through (verified, overlaid, on a scoped credential).
//! - `airhouse_duckdb`: the in-process DuckDB stand-in for every preview port,
//!   a test double never wired into the server (hidden from the docs).
//!
//! Then, on 2026-11-22 (W4), the core: the project context and what is keyed
//! on it (`oxy-app` re-exports each at its old path:
//! `agentic_wiring::{project_ctx, compile_dispatcher}`,
//! `server::preagg_executor`, `server::router::workspace_cache`):
//!
//! - [`project_ctx`]: `OxyProjectContext`, the `ProjectContext` and
//!   `WorkspaceContext` the agentic pipeline runs on.
//! - [`compile_dispatcher`]: the `CompileDispatcher` a `TaskSpec::Compile`
//!   runs through, into the compile worker.
//! - [`preagg_executor`]: the `preagg_cycle` task executor, with the
//!   `CycleWorkspace` impl for `OxyProjectContext` (the orphan rule keeps it
//!   beside that type).
//! - [`workspace_cache`]: `WorkspaceContextCache`, the recovery path's TTL
//!   cache of built contexts.
//!
//! Then, on 2026-11-22 (W5), the preview platform, the last of
//! `agentic_wiring` (`oxy-app` re-exports each at its old path:
//! `agentic_wiring::preview_ctx`, `server::previews::runtime`):
//!
//! - [`preview_ctx`]: `PreviewPlatformContext`, the `PlatformContext` a
//!   workspace preview's procedure dry run and Airway sample run on (reads at
//!   the staging revision, writes held or confined to the preview's schemas).
//! - [`preview_runtime`]: `PreviewRunResolver`, the global-run driver's
//!   `RunPlatformResolver` that hands a preview-owned root that platform.
//!
//! Then, on 2026-11-22 (W6a), the custom-app data plane's composition root
//! (`oxy-app` re-exports each at its old path for its `tests/`:
//! `server::api::custom_apps_gates`, `server::api::projects::semantic_boundary`):
//!
//! - [`custom_apps_gates`]: the gate chain answered as a response,
//!   `parse_versioned_body`, and `build_project_context*` /
//!   `build_caller_context`, which compose the data plane's `OxyProjectContext`.
//! - [`semantic_boundary`]: the prelude the semantic-analysis twins (metric
//!   tree, world model, cohort) enter by: the gate, the context and the scan.
//!
//! Then (decompose W7, 2026-11-22) the workspace request, [`request`]: the
//! workspace middleware that builds an `OxyProjectContext` per request, the Factory-access
//! gate it applies, and the preview read-only guard's axum adapter.
//!
//! Then, on 2026-11-23 (W6b), the queued analytics run and the driver's
//! context builders (`oxy-app` re-exports each at its old path:
//! `server::queued_analytics`, and `server::router::recovery`'s
//! `build_workspace_ctx` and `worker_ctx`):
//!
//! - [`queued_analytics`]: the claim-safe attempt every executor of a queued
//!   analytics run shares (admission, heartbeat, `Settled`), the chat run's
//!   executor, and who a driven run runs as (`caller`, `requester`).
//! - [`driver_ctx`]: the `OxyProjectContext` a run driver, a health or
//!   reconciliation sweep and a requester's run build with no request behind
//!   them, and the driver's revision-tagged cache of it (`worker_ctx`).

/// Test stand-in for Airhouse; never wired into the server (see its module doc).
#[doc(hidden)]
pub mod airhouse_duckdb;
pub mod airhouse_pool;
pub mod app_airhouse;
pub mod compile_dispatcher;
pub mod connector;
pub mod custom_apps_gates;
pub mod driver_ctx;
pub mod memory_merge_executor;
pub mod metric_sink;
pub mod metric_tree_exec;
pub mod metric_tree_runner;
pub mod preagg_executor;
pub mod preagg_workspace;
pub mod preview_airhouse;
pub mod preview_ctx;
pub mod preview_runtime;
pub mod project_ctx;
pub mod queued_analytics;
pub mod react_results;
mod read_only_airhouse;
pub mod request;
pub mod runtime_artifact;
pub mod scan_source;
pub mod semantic_boundary;
pub mod sql_exec;
pub mod sql_http;
pub mod string_literal;
#[cfg(test)]
mod test_support;
pub mod thread_owner;
pub mod typed_stream;
pub mod workspace_cache;
