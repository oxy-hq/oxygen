//! The compile boundary's runtime half: what runs a compile and keeps its
//! `revisions` table, above the compiler (`oxy-compile`) and below `oxy-app`.
//!
//! Out of `oxy-app`'s `server::compile_*` on 2026-11-21 (the decompose's C1),
//! re-exported at the old paths (`oxy_app::server::{compile_git, compile_oltp,
//! compile_config_gate, compile_maintenance}`), so no caller changed:
//!
//! - [`git`]: fetch a commit as a tarball, unpack it safely and hand the
//!   directory to the compiler (a compile with no working copy), the
//!   `compile_git` task, and the branch self-heal plan.
//! - [`config_gate`]: the `config.yml` deserialisation gate the compiler calls
//!   through `oxy_compile::ConfigGate`.
//! - [`oltp`]: apply a revision's OLTP schemas, then promote it.
//! - [`maintenance`]: the stuck-compile reaper and the revision retention loop.
//!
//! It is its own crate, not part of `oxy-compile`, because it needs
//! `oxy-app-core` (the process role, the workspace's repository facts), and
//! `oxy-app-core` depends on `oxy-compile`. The rest of `server::compile_*`
//! (`compile_request`, `compile_reconcile`, `compile_worker`) follows it:
//! `internal-docs/domain-boundaries.md`, End state.

pub mod config_gate;
pub mod git;
pub mod maintenance;
pub mod oltp;
