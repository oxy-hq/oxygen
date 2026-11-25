//! Step 8: `oxy_changes`, the store's change log and the DDL guard that holds
//! its capture (OLTP change stream design §5.2).
//!
//! The owner owns the schema and everything in it, and **no writer is granted
//! anything here**: not `USAGE`, not `EXECUTE`. Capture is written by definer
//! functions the owner puts in each app's own schema at publish
//! ([`crate::changes`]), so the log is out of an app's reach whatever SQL it
//! runs, and the analyst does not see it either.
//!
//! - `stream` holds one row, the store's: its generation, whether it
//!   captures, and the relay's batch numbers. This step inserts **no** row.
//!   The first install seeds it, and a branch cut seeds its own; a capture
//!   function finding none fails the write rather than going uncaptured.
//! - `seq` is `CACHE 1`: a larger cache lets a backend hold a block of lower
//!   numbers, and per-row order is `seq` order only without one.
//! - `log` is the change log, read and trimmed by the relay.
//! - `tables` says what this store captures, by oid, so a rename cannot slip
//!   a table out of it; `shapes` keeps each capture shape a table has had, so
//!   the relay reads a row's columns through the shape it was written with.
//!
//! The DDL guard is [`guard`]: created where the owner may create event
//! triggers (Neon, through `neon_superuser`), skipped where it may not (the
//! local provider), and the step succeeds either way.

use super::{
    CHANGES_BATCH_SEQUENCE, CHANGES_LOG_TABLE, CHANGES_PROGRESS_TABLE, CHANGES_SCHEMA,
    CHANGES_SEQUENCE, CHANGES_SHAPES_TABLE, CHANGES_STREAM_TABLE, CHANGES_TABLES_TABLE,
    PlatformStep,
};

mod guard;

/// Step 8, as the module docs describe it. `analyst` is the tenant's analyst
/// role, already quoted, revoked from the schema as step 5 revokes it from
/// `oxy_events`.
pub(super) fn changes_step(analyst: &str) -> PlatformStep {
    let mut statements = vec![
        format!("CREATE SCHEMA IF NOT EXISTS {CHANGES_SCHEMA}"),
        format!("REVOKE ALL ON SCHEMA {CHANGES_SCHEMA} FROM PUBLIC"),
        format!("REVOKE ALL ON SCHEMA {CHANGES_SCHEMA} FROM {analyst}"),
        format!(
            "CREATE TABLE IF NOT EXISTS {CHANGES_STREAM_TABLE} (
  one boolean PRIMARY KEY DEFAULT true CHECK (one),
  generation bigint NOT NULL,
  capturing boolean NOT NULL,
  intent_batch bigint NOT NULL DEFAULT 0,
  shipped_batch bigint NOT NULL DEFAULT 0)"
        ),
        format!("CREATE SEQUENCE IF NOT EXISTS {CHANGES_SEQUENCE} CACHE 1"),
        format!(
            "CREATE TABLE IF NOT EXISTS {CHANGES_LOG_TABLE} (
  seq bigint NOT NULL DEFAULT nextval('{CHANGES_SEQUENCE}'),
  generation bigint NOT NULL,
  txid xid8 NOT NULL DEFAULT pg_current_xact_id(),
  writer text NOT NULL,
  table_name text NOT NULL,
  schema_version int NOT NULL,
  op \"char\" NOT NULL,
  key jsonb,
  row_image jsonb,
  actor text,
  invocation uuid,
  provenance bytea,
  changed_at timestamptz NOT NULL DEFAULT clock_timestamp())"
        ),
        format!("CREATE INDEX IF NOT EXISTS log_seq ON {CHANGES_LOG_TABLE} (generation, seq)"),
        format!(
            "CREATE TABLE IF NOT EXISTS {CHANGES_TABLES_TABLE} (
  relid oid PRIMARY KEY,
  schema_name text NOT NULL,
  table_name text NOT NULL,
  writer text NOT NULL,
  schema_version int NOT NULL,
  installed boolean NOT NULL)"
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS {CHANGES_SHAPES_TABLE} (
  relid oid NOT NULL,
  schema_version int NOT NULL,
  columns jsonb NOT NULL,
  primary_key text[] NOT NULL,
  excluded text[] NOT NULL,
  recorded_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (relid, schema_version))"
        ),
    ];
    statements.extend(guard::guard_statements());
    PlatformStep {
        version: 8,
        description: "change log, written by capture triggers, held by a DDL guard",
        statements,
    }
}

/// Step 9: the relay's per-writer progress. `progress` holds each writer's
/// intended and shipped batch, `batches` numbers them, `stream.restamping`
/// is set only while a generation bump re-stamps, and `log_writer` and
/// `log_writer_txid` serve the relay's per-writer reads. Step 8's `stream.intent_batch` and
/// `shipped_batch` are superseded and left unused. Nothing is revoked: the
/// schema's revokes from step 8 already cover new objects.
pub(super) fn relay_step() -> PlatformStep {
    PlatformStep {
        version: 9,
        description: "per-writer relay progress, batch numbers and the writer index",
        statements: vec![
            format!(
                "CREATE TABLE IF NOT EXISTS {CHANGES_PROGRESS_TABLE} (
  writer text PRIMARY KEY,
  intent_batch bigint NOT NULL DEFAULT 0,
  shipped_batch bigint NOT NULL DEFAULT 0)"
            ),
            format!("CREATE SEQUENCE IF NOT EXISTS {CHANGES_BATCH_SEQUENCE} CACHE 1"),
            format!(
                "ALTER TABLE {CHANGES_STREAM_TABLE} ADD COLUMN IF NOT EXISTS restamping bigint"
            ),
            format!(
                "CREATE INDEX IF NOT EXISTS log_writer ON {CHANGES_LOG_TABLE} (writer, generation, seq)"
            ),
            format!(
                "CREATE INDEX IF NOT EXISTS log_writer_txid ON {CHANGES_LOG_TABLE} (writer, generation, txid)"
            ),
        ],
    }
}

#[cfg(test)]
mod tests {
    use crate::platform::{CHANGES_SEQUENCE, PLATFORM_SCHEMA_VERSION, platform_steps};

    /// Step 8's statements alone.
    fn step_eight() -> Vec<String> {
        let step = platform_steps("local", "oxy_org_x").unwrap().remove(7);
        assert_eq!(step.version, 8);
        step.statements
    }

    #[test]
    fn step_nine_is_the_last_step() {
        let steps = platform_steps("neon", "neondb").unwrap();
        assert_eq!(steps[7].version, 8);
        assert_eq!(steps[8].version, 9);
        assert_eq!(steps.len(), 9);
        assert_eq!(PLATFORM_SCHEMA_VERSION, 9);
    }

    #[test]
    fn step_nine_adds_progress_batches_restamping_and_the_writer_index() {
        let step = platform_steps("local", "oxy_org_x").unwrap().remove(8);
        assert_eq!(step.version, 9);
        let expected = [
            "CREATE TABLE IF NOT EXISTS oxy_changes.progress (
  writer text PRIMARY KEY,
  intent_batch bigint NOT NULL DEFAULT 0,
  shipped_batch bigint NOT NULL DEFAULT 0)",
            "CREATE SEQUENCE IF NOT EXISTS oxy_changes.batches CACHE 1",
            "ALTER TABLE oxy_changes.stream ADD COLUMN IF NOT EXISTS restamping bigint",
            "CREATE INDEX IF NOT EXISTS log_writer ON oxy_changes.log (writer, generation, seq)",
            "CREATE INDEX IF NOT EXISTS log_writer_txid ON oxy_changes.log (writer, generation, txid)",
        ];
        for statement in expected {
            assert!(
                step.statements.iter().any(|s| s == statement),
                "missing {statement}: {:?}",
                step.statements
            );
        }
    }

    /// The log is the owner's alone: capture reaches it through definer
    /// functions written at publish, never through a grant made here.
    #[test]
    fn step_eight_grants_writers_nothing() {
        for stmt in step_eight() {
            assert!(!stmt.contains("GRANT"), "{stmt}");
        }
    }

    /// Per-row order is `seq` order only when no backend holds a cached block
    /// of lower numbers.
    #[test]
    fn the_sequence_caches_one() {
        let statements = step_eight();
        let sequence = statements
            .iter()
            .find(|s| s.starts_with("CREATE SEQUENCE"))
            .expect("the sequence");
        assert_eq!(
            sequence,
            &format!("CREATE SEQUENCE IF NOT EXISTS {CHANGES_SEQUENCE} CACHE 1")
        );
        let log = statements
            .iter()
            .find(|s| s.starts_with("CREATE TABLE IF NOT EXISTS oxy_changes.log"))
            .expect("the log");
        assert!(
            log.contains(&format!("DEFAULT nextval('{CHANGES_SEQUENCE}')")),
            "{log}"
        );
    }

    /// `one` is the key and can only be true, so the table holds one row or
    /// none; the step inserts none.
    #[test]
    fn stream_holds_at_most_one_row() {
        let statements = step_eight();
        let stream = statements
            .iter()
            .find(|s| s.starts_with("CREATE TABLE IF NOT EXISTS oxy_changes.stream"))
            .expect("the stream");
        assert!(
            stream.contains("one boolean PRIMARY KEY DEFAULT true CHECK (one)"),
            "{stream}"
        );
        assert!(
            !statements
                .iter()
                .any(|s| s.contains("INSERT INTO oxy_changes.stream")),
            "the step seeds no stream: {statements:?}"
        );
    }

    /// The analyst does not see the log, as it does not see the outbox.
    #[test]
    fn the_analyst_cannot_read_the_log() {
        let statements = step_eight();
        assert!(
            statements.contains(&format!(
                r#"REVOKE ALL ON SCHEMA oxy_changes FROM "{}""#,
                crate::schema::analyst_role_for("local", "oxy_org_x")
            )),
            "{statements:?}"
        );
        assert!(
            statements.contains(&"REVOKE ALL ON SCHEMA oxy_changes FROM PUBLIC".to_string()),
            "{statements:?}"
        );
    }
}
