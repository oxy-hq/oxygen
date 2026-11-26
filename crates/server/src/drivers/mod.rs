//! The background drivers: what drives queued runs and the periodic work of a
//! process that runs workers, started by `oxy serve`'s boot
//! (`commands::serve_background`) and by `oxy worker`.
//!
//! - [`recovery`]: the global driver. The startup pass, the periodic loop, the
//!   latency worker, the TaskSpec registry, the shutdown-hook registry and the
//!   in-process gate (`OXY_INPROC_GLOBAL_WORKER`).
//! - [`periodic`]: the cloud periodic tick. It claims due workspaces from
//!   `workspace_periodic_ticks` and drives each one; `periodic::metrics` is what
//!   the worker's `GET /metrics` appends.
//! - `drive_policy`: which queued kinds this process drives.
//! - `run_retention`: deletes finished runs past `OXY_RUN_RETENTION_DAYS`.
//! - [`fleet_jobs`]: the once-per-interval jobs, with the audit anchor
//!   (`audit_anchor`) as the binary's own.
//! - [`token_usage_flush`]: this process's per-token usage counters, written
//!   once a minute and on shutdown.
//! - `billing_reconciler`: the Stripe seat reconciler, disabled.
//!
//! All of it was `oxy-app`'s (`server::router::{recovery, periodic,
//! drive_policy, run_retention, fleet_jobs}` and `server::{audit_anchor,
//! token_usage_flush}`) until decompose BL1 (2026-11-23) moved it up here,
//! unchanged, once BL-W had made `serve` start the boot jobs itself. The layout
//! under `router/` is kept, so the drivers' `include_str!` wiring tests read
//! the same relative paths.

mod audit_anchor;
mod billing_reconciler;
mod drive_policy;
pub(crate) mod fleet_jobs;
pub(crate) mod periodic;
mod preview_tick;
pub(crate) mod recovery;
mod run_retention;
#[cfg(test)]
mod test_support;
pub(crate) mod token_usage_flush;

// The driver's context cache, `oxy-workspace-runtime`'s since decompose W4.
// Named here so the drivers' `super::workspace_cache::…` paths read as they did
// under `oxy-app`'s `router`.
use oxy_workspace_runtime::workspace_cache;
