//! Subrun-runner contract + shared helpers for cross-domain sub-execution.
//!
//! A "subrun" is an execution that one domain delegates to another. The
//! analytics domain, for example, can hand off to an automation
//! when it determines a verified multi-step path exists. The types in
//! this module form the abstract contract between the delegating domain
//! and the executing one — they live in core so that no domain has to
//! depend on another domain to talk about subruns.
//!
//! - [`SubrunRunner`] — adapter trait that delegating domains use to
//!   discover available subruns (search). Execution itself goes through
//!   the coordinator/worker path, not this trait.
//! - [`SubrunRef`] / [`SubrunOutput`] / [`SubrunStepResult`] /
//!   [`SubrunError`] — value types crossing the trait boundary.
//! - [`SubrunStep`] — the lightweight step descriptor that executors
//!   emit in the `subrun_started` event so consuming UIs can render the
//!   full DAG before per-step events arrive.
//! - [`OxyCommentBlock`] / [`parse_oxy_comment_block`] — the leading
//!   `/* oxy: ... */` block parser shared between the subrun search
//!   path (ranking verified SQL files) and any future caller that
//!   needs to read the same convention.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ── Subrun-runner I/O ──────────────────────────────────────────────────────

/// A reference to a discoverable subrun file (e.g. a `.procedure.yml`).
#[derive(Debug, Clone)]
pub struct SubrunRef {
    /// Display name (typically the file stem, e.g. `"monthly_revenue"`).
    pub name: String,
    /// **Workspace-relative** path to the subrun file, e.g.
    /// `"workflows/sales/monthly_revenue.procedure.yml"`. The downstream
    /// `DelegationTarget::Automation { workflow_ref }` +
    /// `WorkspaceContext::resolve_automation_yaml` contract both reject
    /// absolute paths (containment guard against `..`-traversal), so
    /// callers that synthesize `SubrunRef`s must strip any workspace
    /// prefix before populating this field.
    pub path: PathBuf,
    /// Short human-readable description scraped from the file, if available.
    pub description: String,
}

/// Pre-extracted result for a single subrun step.
///
/// Table steps carry real columns and typed rows; non-table steps carry a
/// single `"result"` column with the text representation. Rows are
/// `Vec<Vec<serde_json::Value>>` so numeric columns survive as JSON
/// numbers rather than strings.
#[derive(Debug, Clone)]
pub struct SubrunStepResult {
    /// Task name from the subrun definition.
    pub step_name: String,
    /// Column names — single element `["result"]` for non-table steps.
    pub columns: Vec<String>,
    /// Typed row data (already truncated by the runner).
    pub rows: Vec<Vec<serde_json::Value>>,
    /// Whether the table rows were truncated by the runner.
    pub truncated: bool,
    /// Total number of rows (equals `rows.len()` when not truncated).
    pub total_row_count: u64,
}

/// Raw output returned by the runner after executing a subrun.
#[derive(Debug, Clone)]
pub struct SubrunOutput {
    /// One entry per top-level step in execution order.
    pub steps: Vec<SubrunStepResult>,
}

/// Error returned by the runner.
#[derive(Debug)]
pub struct SubrunError(pub String);

impl std::fmt::Display for SubrunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "subrun runner error: {}", self.0)
    }
}

impl std::error::Error for SubrunError {}

/// Adapter for subrun search and discovery.
///
/// Delegating domains use this to find existing subruns (e.g. via a
/// `search_automations` tool). Subrun *execution* is delegated to the
/// coordinator-worker architecture, not this trait.
#[async_trait::async_trait]
pub trait SubrunRunner: Send + Sync {
    /// Search for existing subruns matching `query`. Returns an empty
    /// `Vec` when no runner is configured or no matches are found.
    async fn search(&self, query: &str) -> Vec<SubrunRef>;
}

// ── Step-info event payload ────────────────────────────────────────────────

/// Describes a single top-level step in a subrun definition.
///
/// Emitted in the `subrun_started` event payload so the frontend can
/// render the full DAG with idle steps before any per-step events arrive.
///
/// Container steps (`loop_sequential`, `workflow`) carry their child
/// steps in [`inner_tasks`](Self::inner_tasks) recursively, so a single
/// `subrun_started` event ships the full nested DAG. The frontend uses
/// `inner_tasks` to drive per-task-type rendering inside loop
/// iterations and sub-automation expansions instead of dumping raw JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubrunStep {
    /// Human-readable step name from the subrun definition.
    pub name: String,
    /// Step type string (e.g. `"execute_sql"`, `"loop_sequential"`).
    pub task_type: String,
    /// Child steps for container task types.
    ///
    /// - `loop_sequential` — the loop body's task definitions.
    /// - `workflow` — the referenced child automation's tasks (recursive).
    /// - all others — empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inner_tasks: Vec<SubrunStep>,
}

// ── Oxy comment block parser ───────────────────────────────────────────────

/// Parsed contents of the leading `/* oxy: ... */` block in a SQL file.
///
/// Both fields are optional; missing or empty values become `None`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OxyCommentBlock {
    /// Human-readable description used by subrun search to rank the file.
    pub description: Option<String>,
    /// Logical connector name to execute the SQL against, overriding the
    /// agent's default connector.
    pub database: Option<String>,
}

/// Parse the leading `/* oxy: ... */` comment block of a SQL file.
///
/// Recognises:
///
/// ```sql
/// /*
///   oxy:
///     description: "Monthly revenue by region"
///     database: my_db
/// */
/// ```
///
/// Returns `None` when the file has no leading comment block, or when the
/// block doesn't contain an `oxy:` key. The returned [`OxyCommentBlock`]
/// only carries fields that are present and non-empty.
pub fn parse_oxy_comment_block(content: &str) -> Option<OxyCommentBlock> {
    let start = content.find("/*")?;
    // Searched after the opener, so `/*/` is not read as an empty block.
    let end_offset = content[start + 2..].find("*/")?;
    let comment = &content[start + 2..start + 2 + end_offset];

    #[derive(Deserialize)]
    struct OxyInner {
        description: Option<String>,
        database: Option<String>,
    }
    #[derive(Deserialize)]
    struct OxyComment {
        oxy: Option<OxyInner>,
    }
    let inner = serde_yaml::from_str::<OxyComment>(comment).ok()?.oxy?;
    let block = OxyCommentBlock {
        description: inner.description.filter(|s| !s.is_empty()),
        database: inner.database.filter(|s| !s.is_empty()),
    };
    if block == OxyCommentBlock::default() {
        None
    } else {
        Some(block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_block() {
        let sql =
            "/*\n  oxy:\n    description: \"Monthly revenue\"\n    database: my_db\n*/\nSELECT 1;";
        let block = parse_oxy_comment_block(sql).unwrap();
        assert_eq!(block.description.as_deref(), Some("Monthly revenue"));
        assert_eq!(block.database.as_deref(), Some("my_db"));
    }

    #[test]
    fn returns_none_without_block() {
        assert!(parse_oxy_comment_block("SELECT 1;").is_none());
    }

    #[test]
    fn returns_none_when_block_has_no_oxy_key() {
        let sql = "/* not an oxy block */ SELECT 1;";
        assert!(parse_oxy_comment_block(sql).is_none());
    }

    #[test]
    fn an_opener_that_touches_a_closer_is_not_a_block() {
        // Once a panic: the closer was searched from the opener's own `/`.
        assert!(parse_oxy_comment_block("/*/ SELECT 1").is_none());
    }

    #[test]
    fn empty_string_fields_treated_as_none() {
        let sql = "/*\n  oxy:\n    description: \"\"\n    database: \"\"\n*/\nSELECT 1;";
        assert!(parse_oxy_comment_block(sql).is_none());
    }
}
