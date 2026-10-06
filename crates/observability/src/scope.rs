//! Whose traces a read may return.
//!
//! Every tenant's agent spans share one ClickHouse database, and until this
//! module the reads over them took no tenant predicate at all: the
//! `{workspace_id}` in the route authorized the caller and was then dropped,
//! so each workspace's Traces console listed every workspace's runs — prompts
//! and SQL included.
//!
//! The fix has two halves that only work together:
//!
//! - **Write.** A root span says which workspace its run belongs to
//!   ([`WORKSPACE_ATTRIBUTE`]), the collector layer hands that down to every
//!   span beneath it, and each row is stored with it (`workspace_id`).
//! - **Read.** A tenant-facing read takes a [`WorkspaceScope`] and can only
//!   name rows stamped with that workspace.
//!
//! **Unstamped is nobody's.** A row written before the column existed, by a
//! binary that predates this, or under a root that never said whose it was,
//! has `workspace_id = ''`. No scope matches the empty string, so such a row
//! is in no workspace's console rather than in everyone's.

use uuid::Uuid;

/// The span field a root span sets to claim its trace for a workspace. Read
/// by the collector layer; see [`crate::layer`].
pub const WORKSPACE_ATTRIBUTE: &str = "oxy.workspace_id";

/// The workspace a trace read is confined to.
///
/// Built only from a [`Uuid`], so the text interpolated into SQL is
/// hyphenated lower-case hex by construction — there is no value of this type
/// that needs escaping, and none that is empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceScope(String);

impl WorkspaceScope {
    pub fn of(workspace_id: Uuid) -> Self {
        Self(workspace_id.to_string())
    }

    /// The workspace id as rows carry it.
    pub fn workspace_id(&self) -> &str {
        &self.0
    }

    /// The predicate that keeps a read of `observability_spans` inside this
    /// workspace. `alias` is the table alias with its dot (`"s."`), or `""`.
    pub(crate) fn spans_predicate(&self, alias: &str) -> String {
        format!("{alias}workspace_id = '{}'", self.0)
    }

    /// This workspace's trace ids, for a table that has a `trace_id` but no
    /// tenant column of its own: `trace_id IN ({…})`. Root rows only, so the
    /// set is one id per run rather than one per span.
    pub(crate) fn trace_ids(&self) -> String {
        format!(
            "SELECT trace_id FROM observability_spans WHERE {} AND parent_span_id = ''",
            self.spans_predicate("")
        )
    }

    /// [`Self::trace_ids`] for runs that started in the last `days`, so a
    /// windowed read does not walk the whole retention to find its own traces.
    ///
    /// One day wider than asked: a root opens before the rows its run
    /// produces, and a run is minutes long, never a day. A row from a run that
    /// somehow outlived the slack is left out — hidden, not leaked.
    pub(crate) fn trace_ids_within(&self, days: u32) -> String {
        format!(
            "{} AND timestamp >= now() - INTERVAL {} DAY",
            self.trace_ids(),
            days.saturating_add(1)
        )
    }

    /// The predicate that keeps a read of a rollup — `observability_executions`,
    /// `observability_metric_usage` — inside this workspace, for rows of the
    /// last `days`. Those tables have a `trace_id` and no tenant column, so a
    /// row is this workspace's when its trace is. A row with no trace id
    /// matches nothing and is nobody's.
    pub(crate) fn rollup_predicate(&self, days: u32) -> String {
        format!("trace_id IN ({})", self.trace_ids_within(days))
    }
}

/// The workspace a span field names, in the one spelling rows and scopes
/// share, or `None` when it is not a workspace id.
///
/// The collector records fields through `Debug`, so a `%uuid` arrives bare and
/// a `&str` may arrive quoted; both are the same workspace. Anything that does
/// not parse is treated as absent — a row is unstamped before it is stamped
/// with a string no scope could ever equal by accident of formatting.
pub(crate) fn stamped_workspace(raw: &str) -> Option<String> {
    Uuid::parse_str(raw.trim().trim_matches('"'))
        .ok()
        .map(|id| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKSPACE: &str = "70787bb2-e11b-5488-b2c3-02e60d5fc7d3";

    fn scope() -> WorkspaceScope {
        WorkspaceScope::of(Uuid::parse_str(WORKSPACE).unwrap())
    }

    #[test]
    fn a_scope_names_exactly_its_workspace() {
        assert_eq!(scope().workspace_id(), WORKSPACE);
        assert_eq!(
            scope().spans_predicate("s."),
            format!("s.workspace_id = '{WORKSPACE}'")
        );
        assert_eq!(
            scope().spans_predicate(""),
            format!("workspace_id = '{WORKSPACE}'")
        );
    }

    /// The whole point of "unstamped is nobody's": the value a scope compares
    /// against can never be the empty string an unstamped row carries — not
    /// even for the nil workspace legacy local mode runs under.
    #[test]
    fn no_scope_matches_an_unstamped_row() {
        for id in [Uuid::nil(), Uuid::parse_str(WORKSPACE).unwrap()] {
            let scope = WorkspaceScope::of(id);
            assert!(!scope.workspace_id().is_empty());
            assert!(!scope.spans_predicate("").ends_with("= ''"));
        }
    }

    /// The set a rollup read joins against is this workspace's roots and
    /// nothing wider: without the workspace predicate it is every tenant's
    /// trace ids, which is the bug.
    #[test]
    fn trace_ids_are_this_workspaces_roots() {
        let sql = scope().trace_ids();
        assert!(
            sql.contains(&format!("workspace_id = '{WORKSPACE}'")),
            "{sql}"
        );
        assert!(sql.contains("parent_span_id = ''"), "{sql}");
        assert!(sql.starts_with("SELECT trace_id FROM observability_spans WHERE"));
    }

    #[test]
    fn a_rollup_is_confined_through_this_workspaces_recent_traces() {
        let sql = scope().rollup_predicate(30);
        assert!(
            sql.starts_with(&format!("trace_id IN ({}", scope().trace_ids())),
            "{sql}"
        );
        // A day of slack past the window that was asked for.
        assert!(sql.ends_with("INTERVAL 31 DAY)"), "{sql}");
        // The widest window a caller can name still produces a window.
        assert!(
            scope()
                .rollup_predicate(u32::MAX)
                .ends_with(&format!("INTERVAL {} DAY)", u32::MAX))
        );
    }

    #[test]
    fn a_stamp_is_read_in_every_spelling_the_collector_produces() {
        // `%uuid` → bare; a `&str` field recorded through `Debug` → quoted;
        // upper case and padding are the same workspace.
        for raw in [
            WORKSPACE.to_string(),
            format!("\"{WORKSPACE}\""),
            WORKSPACE.to_uppercase(),
            format!("  {WORKSPACE} "),
        ] {
            assert_eq!(stamped_workspace(&raw).as_deref(), Some(WORKSPACE), "{raw}");
        }
    }

    #[test]
    fn a_stamp_that_is_not_a_workspace_id_is_no_stamp() {
        for raw in ["", "   ", "\"\"", "demo", "' OR '1'='1", "70787bb2"] {
            assert_eq!(stamped_workspace(raw), None, "{raw:?}");
        }
    }
}
