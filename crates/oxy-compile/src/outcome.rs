//! What `compile_workspace` returns + the per-file detail it records
//! into the `revisions` row's `error_summary` JSONB.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Result of a full compile run. Whether `status` is `Ready` or
/// `Failed`, the `revisions` row was written successfully and the
/// per-entity tables hold the rows for every file that compiled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompileOutcome {
    pub revision_id: Uuid,
    pub status: RevisionStatus,
    pub git_sha: String,
    pub branch: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub file_count_seen: u32,
    pub file_count_compiled: u32,
    pub file_count_failed: u32,
    /// Per-file failure details. Empty when status is Ready.
    pub failures: Vec<FileFailure>,
    /// What happened to `workspaces.current_revision_id`. See [`Promotion`].
    pub promotion: Promotion,
}

/// What this compile did to `workspaces.current_revision_id`.
///
/// Before this existed a caller could not tell "promoted" from "asked to
/// promote and silently lost the causality race" — both returned `Ok(())`
/// and the difference lived only in a log line. Anything that has to know
/// whether *this* revision is the live one needs the distinction, and
/// `Deferred` needs it most: it is the caller's cue to do work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Promotion {
    /// Not asked for (`promote: false`), or withheld because the revision
    /// is a draft or the compile failed. `current_revision_id` untouched
    /// by design — an `oxy compile` against a working tree lands here.
    NotRequested,
    /// `current_revision_id` now points at this revision.
    Promoted,
    /// Requested, but `current_revision_id` was not moved to this
    /// revision. Two ways to get here, and neither is an error worth
    /// failing a compile over: the `started_at` causality clause in
    /// `promote_revision` made the UPDATE a no-op because a newer revision
    /// is already current, or the promote of a superseded-path winner
    /// failed and was logged. Both mean the same thing to a reader — this
    /// revision is not the one being served — and its rows stay queryable
    /// by `revision_id` either way.
    Skipped,
    /// Requested and **deliberately withheld**: this revision carries
    /// `schemas/*.sql` DDL that must reach the org's OLTP database before
    /// the runtime starts reading a revision whose tables may not exist.
    ///
    /// The caller owns the apply-then-promote step. It cannot live in this
    /// crate for two reasons: applying is a network round-trip to a tenant
    /// database, which must never happen inside the finalise transaction;
    /// and `oxy-compile` does not depend on `oxy-oltp` (they are siblings
    /// over `entity`), which is the boundary that keeps the compiler
    /// ignorant of where a tenant's Postgres lives.
    ///
    /// Callers: `oxy_app::server::compile_worker` and `oxy compile`.
    Deferred { schema_migration_count: u32 },
}

impl Promotion {
    /// True when `current_revision_id` points at this revision *now*.
    ///
    /// `Deferred` is false: the whole point is that it is not live yet.
    pub fn is_live(&self) -> bool {
        matches!(self, Promotion::Promoted)
    }

    /// The migration count when promotion is waiting on DDL, else `None`.
    pub fn deferred_count(&self) -> Option<u32> {
        match self {
            Promotion::Deferred {
                schema_migration_count,
            } => Some(*schema_migration_count),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisionStatus {
    Ready,
    Failed,
}

impl RevisionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            RevisionStatus::Ready => "ready",
            RevisionStatus::Failed => "failed",
        }
    }
}

/// Per-file failure recorded into the revision's `error_summary`. Plain
/// JSON so the admin UI can read it back without a typed deserialize.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileFailure {
    /// Workspace-relative path. Forward slashes regardless of OS so
    /// the JSON is portable.
    pub path: String,
    /// Compile-time classification. Lets the UI group by failure
    /// shape (the same trick the Internal Jobs dead-letter inspector
    /// uses).
    pub kind: FailureKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// `serde_yaml::from_str` returned an error — the YAML is
    /// malformed.
    Yaml,
    /// File could not be read from disk.
    Io,
    /// The YAML parsed but required field was missing (e.g. an
    /// automation with no top-level structure).
    Shape,
    /// Two files of the same kind compiled to the same identifier
    /// (e.g. two agents named "foo"). The failure is recorded on the
    /// later file (discovery is sorted by path) and its message names
    /// the file that declared the identifier first, so the operator
    /// sees both paths.
    Duplicate,
    /// The file parsed and had the right shape, but the compiled output
    /// would not deserialise into the runtime type at read time (e.g. a
    /// `config.yml` whose compiled form fails `from_value::<Config>`).
    /// Recorded as a failure so the revision is NOT promoted — a config
    /// the fleet can't read must never become the active revision. See the
    /// round-trip gate in `compile::drive_compile`.
    Validation,
}
