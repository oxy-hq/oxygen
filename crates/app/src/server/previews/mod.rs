//! Workspace previews: open a branch of a workspace on the real product,
//! against real data, without the branch being live. Staff only.
//!
//! None of it lives in `oxy-app` any more. The rows (registry, DDL, namespace,
//! sources, pin, run queue) are the `oxy-previews` crate (decompose P1,
//! 2026-11-22); the read-only guard's axum adapter, the DuckDB double and the
//! run platform are `oxy-workspace-runtime`'s (W3, W5, W7); the
//! `/{workspace_id}/previews` routes and the checks, compares and samples are
//! `oxy-api-previews`. The design is in the `oxy-previews` crate docs and
//! `internal-docs/customer-apps-staging.md`.
//!
//! What is left is the run platform resolver at its old path, because
//! `tests/platform` names it. The other re-exports here had no caller left and
//! were deleted in decompose SHM (2026-11-23).

/// The run platform resolver, `oxy-workspace-runtime`'s `preview_runtime` since
/// 2026-11-22 (W5), re-exported at its old path.
pub use oxy_workspace_runtime::preview_runtime as runtime;
