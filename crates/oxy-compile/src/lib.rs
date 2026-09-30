//! `oxy-compile` — the write half of the compile boundary.
//!
//! Walks a workspace, parses every recognised YAML/SQL file, and
//! writes the result into the compile-boundary Postgres schema
//! introduced by migration `m20260606_000001_create_compile_boundary`.
//! The result is a single `revisions` row plus per-entity rows
//! (`agent_definitions`, `semantic_views`, `app_definitions`, etc.)
//! all tagged with the new `revision_id`.
//!
//! See `internal-docs/compile-boundary.md` for the operator runbook.
//!
//! **This is live, not observation-mode.** Promotion updates
//! `workspaces.current_revision_id`, and the runtime serves from these
//! rows: `ConfigManager` reads them whenever its `Origin` is
//! `Compiled`, falling through to the working copy only when there is
//! no revision to read. An earlier version of this header described
//! the staged rollout — compiles running while the runtime still read
//! YAML from disk — which stopped being true when the read paths
//! landed.
//!
//! Reading is NOT here. Per-revision queries live in
//! `crates/core/src/config/compiled.rs`, and choosing *which* revision
//! a request reads lives in `oxy-app` (it needs the process role and a
//! `git` call for the default branch). What this crate owns on the
//! read side is the meaning of a row — see
//! [`entity::workspace_compiled_configs::merge_compiled_config`], which
//! is re-exported here and used by both the compile-time gate and the
//! runtime reader so the shape one validated and the other serves
//! cannot drift.
//!
//! Public surface:
//!
//!   - [`compile_workspace`] — the end-to-end entry point used by
//!     `oxy compile`, by the future TaskSpec wrapper, and by tests.
//!   - [`CompileRequest`] / [`CompileOutcome`] — the request +
//!     response types.
//!   - [`errors::CompileError`] — run-level errors (DB unreachable,
//!     workspace missing). Per-file failures are recorded inside the
//!     `CompileOutcome.failures` vector, not raised.
//!
//! Boundaries:
//!
//!   - `walker.rs` is the only module that touches the filesystem.
//!   - `writer.rs` is the only module that touches the database.
//!   - `compile.rs` orchestrates and holds the parsing logic; no I/O
//!     of either kind except via the other two modules.
//!
//! This crate is `oxy-shared`-like: small, focused, no upward
//! dependencies on platform crates. The CLI command lives in
//! `oxy-app` and the future TaskSpec wrapper lives in
//! `agentic-pipeline`.

pub mod blob_store;
pub mod compile;
pub mod duckdb_mirror;
pub mod errors;
pub mod outcome;
pub mod preagg_blob;
pub mod walker;
pub mod workspace_path;
pub mod writer;

pub use compile::{
    CURRENT_SCHEMA_VERSION, CompileRequest, ConfigGate, RevisionKind, build_compiled_config,
    compile_workspace, find_reusable_revision,
};
// The shape and its merge live next to the columns, in `entity`. Re-exported so
// existing `oxy_compile::` call sites keep compiling.
pub use entity::workspace_compiled_configs::{CompiledConfig, merge_compiled_config};
pub use errors::CompileError;
pub use outcome::{CompileOutcome, FailureKind, FileFailure, Promotion, RevisionStatus};
// Apply-then-promote lives in `oxy-app`: this crate withholds promotion for a
// revision carrying `schemas/*.sql` (see [`Promotion::Deferred`]) but cannot
// apply the DDL itself — that is a network call to a tenant database, and
// `oxy-compile` deliberately does not depend on `oxy-oltp`. This is the door
// the applier comes back through.
pub use workspace_path::resolve_workspace_path;
pub use writer::promote_existing;

/// Identifier the compile worker stamps on every `revisions` row so
/// operator dashboards can spot version-skew across a rolling deploy.
/// Sourced from `CARGO_PKG_VERSION` so a release-bumped oxy binary
/// automatically writes a different value without code changes.
pub fn compiler_version() -> String {
    format!("oxy-compile/{}", env!("CARGO_PKG_VERSION"))
}
