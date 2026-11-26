//! The relay's drift check (OLTP change stream design §5.2, layer 3): which
//! declared tables' capture is no longer as an install left it, and whether
//! the store still has its DDL guard. Where the guard holds, a writer cannot
//! drift its capture at all; where the owner may not create it (the local
//! provider), this check is what finds a trigger the app disabled or dropped.
//!
//! Every statement runs as the store's owner, in a transaction whose search
//! path is pinned (`pin_session_sql`). Nothing here opens a connection.

use crate::platform::{CHANGES_GUARD_TRIGGER, CHANGES_TABLES_TABLE, capture_triggers_in_place};

/// `$1 writers text[], $2 tables text[]`: the declared (writer, table)
/// pairs, element by element. One row per pair that drifted, `writer,
/// table_name, drift` (a [`Drift`] code), plus `NULL, NULL, 'no_guard'` when
/// `oxy_changes_guard` is missing or disabled. No rows: nothing drifted.
///
/// A pair is matched to its `tables` row by the name it was installed under,
/// then to the catalog by oid, and classified by the first test it fails, in
/// [`Drift`]'s order. The trigger test is the guard's own
/// ([`capture_triggers_in_place`]).
pub fn drift_sql() -> String {
    let in_place = capture_triggers_in_place("t.relid", "t.schema_name");
    format!(
        "SELECT writer, table_name, drift FROM (
  SELECT d.writer, d.table_name,
         CASE
           WHEN t.relid IS NULL THEN 'not_captured'
           WHEN NOT t.installed THEN 'not_committed'
           WHEN c.oid IS NULL THEN 'dropped'
           WHEN c.relname <> t.table_name OR n.nspname <> t.schema_name THEN 'renamed'
           WHEN {in_place} < 4 THEN 'triggers'
         END AS drift
    FROM unnest($1::text[], $2::text[]) AS d (writer, table_name)
    LEFT JOIN {CHANGES_TABLES_TABLE} t
           ON t.writer = d.writer AND t.table_name = d.table_name
    LEFT JOIN pg_catalog.pg_class c ON c.oid = t.relid
    LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
) pairs
WHERE drift IS NOT NULL
UNION ALL
SELECT NULL::text, NULL::text, 'no_guard'
 WHERE NOT EXISTS (
   SELECT 1 FROM pg_catalog.pg_event_trigger
    WHERE evtname = '{CHANGES_GUARD_TRIGGER}' AND evtenabled IN ('O', 'A'))"
    )
}

/// How a declared table's capture drifted, or that the store lost its guard:
/// a row of [`drift_sql`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drift {
    /// The table has no `tables` row: capture was never installed on it.
    NotCaptured,
    /// An install recorded it and never reached its commit.
    NotCommitted,
    /// The table it was installed on is gone.
    Dropped,
    /// The table it was installed on has another name or schema now.
    Renamed,
    /// Fewer than its four capture triggers are in place.
    Triggers,
    /// `oxy_changes_guard` is missing or disabled.
    NoGuard,
}

impl Drift {
    const ALL: [Drift; 6] = [
        Self::NotCaptured,
        Self::NotCommitted,
        Self::Dropped,
        Self::Renamed,
        Self::Triggers,
        Self::NoGuard,
    ];

    /// The drift [`drift_sql`] names `code`; `None` for any other.
    pub fn from_code(code: &str) -> Option<Drift> {
        Self::ALL.into_iter().find(|drift| drift.as_str() == code)
    }

    /// Whether the relay repairs it: the table has its `tables` row and is
    /// where it was installed, so installing it again as it stands puts its
    /// capture back. Every other drift adds or removes capture of a table,
    /// which is a promote's to do, never the relay's: the relay compares the
    /// store with the build production serves, and a promote installs before
    /// production moves to its build.
    pub fn repairable(self) -> bool {
        matches!(self, Self::Triggers | Self::NotCommitted)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotCaptured => "not_captured",
            Self::NotCommitted => "not_committed",
            Self::Dropped => "dropped",
            Self::Renamed => "renamed",
            Self::Triggers => "triggers",
            Self::NoGuard => "no_guard",
        }
    }
}

/// Record that the store runs without its DDL guard, from now, unless it is
/// already recorded.
pub const GUARDLESS_SQL: &str =
    "UPDATE oxy_changes.stream SET guardless_since = now() WHERE guardless_since IS NULL";

/// Clear [`GUARDLESS_SQL`]'s record, once the guard is found again.
pub const GUARDED_SQL: &str =
    "UPDATE oxy_changes.stream SET guardless_since = NULL WHERE guardless_since IS NOT NULL";
