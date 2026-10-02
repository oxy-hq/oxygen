//! Oxy git client.
//!
//! Single source of truth for git operations. All git CLI invocations,
//! auth injection, and path validation live in this crate. Consumers
//! (handlers, services) depend on the [`GitClient`] trait and never
//! shell out to `git` directly.
//!
//! Auth is always injected via `-c http.extraHeader=Authorization: Bearer …`;
//! tokens are never written into `.git/config` or embedded in remote URLs.

pub mod cli;
pub mod client;
pub mod types;

pub use cli::head::{
    DetachedHead, HeadState, detached_label_refusal, detached_label_sha, is_detached_label,
};
pub use cli::push_pull::PullOutcome;
pub use client::GitClient;
pub use types::{
    Auth, BranchInfo, BranchOrigin, DirtyEntry, DirtyKind, FileStatus, LocalRefOrigin,
    RecentCommit, ResetOutcome,
};
