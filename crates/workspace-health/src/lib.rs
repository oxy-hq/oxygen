//! Workspace health — the cross-tenant rollup staff read in the admin console,
//! and the evaluation that produces it.
//!
//! `evaluator` is pure (signal counts -> status); `queries` gathers signals
//! from shared Postgres; `alert` decides what is worth telling Slack — status
//! transitions, plus a recurring reminder for as long as a workspace stays
//! unhealthy — and pushes it. `history` keeps each status change, which the
//! state row overwrites. `eval_pass` runs one workspace's evaluation end to end,
//! and `rollup` is the read the console serves.
//!
//! # The crate
//!
//! The domain half of a two-crate extraction from `oxy-app`'s
//! `server::api::admin::workspace_health` (`internal-docs/domain-boundaries.md`).
//! It never depends on `oxy-app`: the two things that need it — the live
//! reconcile and smoke runners, which build a workspace's project context — are
//! the [`reconcile::ReconcileRunner`] and [`smoke::SmokeRunner`] seams, filled
//! by `oxy-api-workspace-health`, which also owns the admin routes and the
//! `health_eval_workspace` task executor. The partner console reads
//! [`rollup::health_rollup`] from here directly.

pub mod alert;
pub mod app_availability;
pub mod eval_pass;
pub mod evaluator;
pub mod history;
pub mod queries;
pub mod reconcile;
pub mod rollup;
pub mod smoke;

pub use rollup::{SignalsRow, WorkspaceHealthResponse, WorkspaceHealthRow, health_rollup};

/// `oxy-app`'s in-source DB harness, shared by path so these tests migrate and
/// lock the shared test database exactly the way `oxy-app`'s own do.
#[cfg(test)]
#[path = "../../app/src/server/test_support.rs"]
#[allow(dead_code)]
mod test_support;
