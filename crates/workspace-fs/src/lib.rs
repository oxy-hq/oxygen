//! The `WorkspaceFs` port — the seam between the stateless serving plane and the
//! filesystem-owning workspace environment.
//!
//! Superseded: `internal-docs/factory-retirement.md` retires the server-side working
//! copy, so this port gets no remote implementation (`multi-instance-fleet.md` §9).
//!
//! This crate holds ONLY the contract (the trait + its data types), so **both**
//! planes can depend on it:
//! - the **workspace environment** links `oxy-workspace-env`, which provides the
//!   real, disk-backed `LocalWorkspaceFs` implementation;
//! - the **stateless serving fleet** links a `RemoteWorkspaceFs` implementation
//!   (an HTTP proxy to the environment) and **must not** link `oxy-workspace-env`.
//!
//! Keeping the impl out of the serving binary's dependency graph is the
//! structural guarantee (Rung 2): "touch the disk on a stateless replica" stops
//! being a runtime misclassification (the `/apps/source` class of bug) and
//! becomes a thing you cannot compile.
//!
//! Every route currently classified `IdeOnly` in the app's `role_manifest` is,
//! by definition, an operation that belongs behind this port. The surface grows
//! here one handler at a time during the crate move (Stage 1 of the plan).

use async_trait::async_trait;
use oxy_shared::errors::OxyError;

/// A path inside the workspace working copy, relative to its root.
pub type WorkspacePath = String;

/// Working-copy + git status probe (what `/details`, `/status`, and the file
/// tree need). The serving plane may serve a *degraded* form of this
/// (`branch: None`) when the environment is unreachable — see #2528.
#[derive(Debug, Clone)]
pub struct WorkspaceState {
    pub branch: Option<String>,
    pub dirty: bool,
}

/// Every operation that requires the workspace working copy / `.git` / local
/// state dir. An implementor of this trait is, by construction, a process that
/// owns a disk — which is why only the environment can implement it locally.
///
/// Grouped by concern. This is the **representative** seam, not the exhaustive
/// set — it grows one handler at a time during the crate move.
#[async_trait]
pub trait WorkspaceFs: Send + Sync {
    // ── working-copy files ──────────────────────────────────────────────────
    async fn read_file(&self, path: &str) -> Result<Vec<u8>, OxyError>;
    async fn write_file(&self, path: &str, bytes: &[u8]) -> Result<(), OxyError>;
    async fn list_tree(&self, root: &str) -> Result<Vec<WorkspacePath>, OxyError>;
    async fn delete(&self, path: &str) -> Result<(), OxyError>;

    // ── git on the working copy ─────────────────────────────────────────────
    async fn state(&self) -> Result<WorkspaceState, OxyError>;
    async fn switch_branch(&self, branch: &str) -> Result<(), OxyError>;
    async fn commit(&self, message: &str) -> Result<(), OxyError>;
    async fn pull(&self) -> Result<(), OxyError>;
    async fn push(&self) -> Result<(), OxyError>;

    // ── compile (reads the working copy, writes the compile boundary) ────────
    /// Run a compile in-place and promote it. The serving plane then reads the
    /// resulting `*_definitions` rows from Postgres — it never compiles itself.
    async fn compile(&self) -> Result<(), OxyError>;
}
