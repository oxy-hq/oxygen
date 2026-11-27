//! The compile boundary's runtime half: what runs a compile and keeps its
//! `revisions` table, above the compiler (`oxy-compile`) and below `oxy-app`.
//!
//! Out of `oxy-app`'s `server::compile_*` on 2026-11-21 (the decompose's C1),
//! re-exported at the old paths (`oxy_app::server::{compile_git, compile_oltp,
//! compile_config_gate}`; `compile_maintenance`'s went in decompose SHM), so no
//! caller changed:
//!
//! - [`git`]: fetch a commit as a tarball, unpack it safely and hand the
//!   directory to the compiler (a compile with no working copy), the
//!   `compile_git` task, and the branch self-heal plan.
//! - [`config_gate`]: the `config.yml` deserialisation gate the compiler calls
//!   through `oxy_compile::ConfigGate`.
//! - [`oltp`]: apply a revision's OLTP schemas, then promote it.
//! - [`maintenance`]: the stuck-compile reaper and the revision retention loop.
//! - [`preagg_promote`]: the pre-aggregation tick a promote owes (was
//!   `oxy_preagg::promote`, moved to its only caller on 2026-11-27 so
//!   `oxy-preagg` drops `agentic-pipeline`; `internal-docs/crate-tiers.md`).
//! - [`request`]: the branch or commit compile a person waits on, its status
//!   read, and the compile task enqueue (was `server::compile_request`, moved
//!   on 2026-11-22, the decompose's C2; re-exported at
//!   `oxy_app::server::compile_request`).
//! - [`reconcile`]: keep a remote-backed workspace serving what its default
//!   branch has: the periodic branch check, the GitHub push webhook's
//!   reconciler, the staging env pins, and the deduped automatic compile
//!   enqueue (lazy and import) with its backoff and throttle (was
//!   `server::compile_reconcile`, moved on 2026-11-22, the decompose's C3;
//!   re-exported at `oxy_app::server::compile_reconcile`).
//! - [`worker`]: the compile task's worker, which drives
//!   `oxy_compile::compile_workspace` on the runtime's task channels and runs
//!   what follows a promote (the OLTP settle, the health and pre-aggregation
//!   schedule reconciles, the rollup nudge, the env pins and a preview's
//!   checks, the last through [`ports::PreviewChangeCheck`]) (was `server::compile_worker`, moved on 2026-11-22, the
//!   decompose's C4; its `oxy_app::server::compile_worker` re-export went in
//!   decompose SHM).
//!
//! It is its own crate, not part of `oxy-compile`, because it needs
//! `oxy-app-core` (the process role, the workspace's repository facts), and
//! `oxy-app-core` depends on `oxy-compile`:
//! `internal-docs/domain-boundaries.md`, End state.

pub mod config_gate;
pub mod git;
pub mod maintenance;
pub mod oltp;
pub mod ports;
pub mod preagg_promote;
pub mod reconcile;
pub mod request;
pub mod worker;

/// The shared in-source DB harness (`oxy_test_utils::db`), so `worker`'s tests
/// migrate and lock the shared test database exactly the way `oxy-app`'s own do.
#[cfg(test)]
mod test_support {
    pub(crate) use oxy_test_utils::db::SKIP_MSG;

    /// [`oxy_test_utils::db::test_db`] with the runtime migrator.
    pub(crate) async fn test_db() -> Option<sea_orm::DatabaseConnection> {
        oxy_test_utils::db::test_db::<agentic_runtime::migration::RuntimeMigrator>().await
    }
}
