//! Re-export of the `WorkspaceFs` port (canonical home: the `oxy-workspace-fs`
//! crate) so serving-plane code can refer to it as
//! `crate::server::workspace_fs::WorkspaceFs`.
//!
//! The disk-backed impl (`LocalWorkspaceFs`) lives in `oxy-workspace-env` and is
//! linked only by the environment binary; the serving plane's proxy impl
//! (`RemoteWorkspaceFs`) will live alongside the broker. Keeping the impl out of
//! the serving binary's dependency graph is the Rung-2 guarantee. Superseded by
//! `internal-docs/factory-retirement.md`, which removes the working copy instead.

pub use oxy_workspace_fs::{WorkspaceFs, WorkspacePath, WorkspaceState};
