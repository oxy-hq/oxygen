//! The pre-aggregation runtime's domain half, out of `oxy-app`'s `server`
//! (`internal-docs/domain-boundaries.md`, "End state: no `oxy-app`"): below
//! `oxy-app`, no routes, no entities, no migrator.
//!
//! - [`freshness`] decides whether a rollup is STALE (an `every:` cadence or a
//!   `sql:` probe, through the in-memory cache, the manifest and the ledger).
//! - [`rebuild`] rebuilds one stale rollup: CTAS, pull, manifest and cache.
//! - [`retract`] removes a rollup's artifacts (a zero-row rebuild, a wrong
//!   builder) under the manifest's publish lock.
//! - [`generation`] is the builder-generation lever: the sweep that retracts
//!   what an older builder stored wrongly.
//! - [`worker_config`] is the per-task bag the cycle threads through them.
//!
//! The cycle itself, `PreaggTaskExecutor`, stays in `oxy-app`'s
//! `server::preagg_executor` for now: it builds an `OxyProjectContext` and
//! resolves the compiled scan source, both `oxy-app`'s workspace runtime. What
//! this crate needs of a workspace is the [`WorkspaceContext`] trait's
//! `get_connector`, which is how it reached `OxyProjectContext` before.
//!
//! [`WorkspaceContext`]: agentic_automation::workspace::WorkspaceContext

pub mod freshness;
pub mod generation;
pub mod rebuild;
pub mod retract;
pub mod worker_config;
