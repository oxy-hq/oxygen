//! The DDL guard: the database holds an installed table's capture, since the
//! app that owns the table could otherwise drop, disable or rename it (OLTP
//! change stream design §5.2, layers 1 and 2).
//!
//! An app's writer owns its tables, so it keeps every privilege on them,
//! `TRIGGER` included, and grants cannot stop it removing a capture trigger.
//! Two event triggers do, owned by the store owner, which a writer can
//! neither drop nor disable:
//!
//! - **`oxy_changes_guard`** (`ddl_command_end`) refuses any statement after
//!   which an installed table has lost its name, its schema, or one of its
//!   four statement-level capture triggers, each `ENABLE ALWAYS`, unfiltered,
//!   on its own event with its own transition tables, and calling the
//!   function generated for that table; or after which any table's trigger
//!   calls a capture function generated for another table. It holds for
//!   every role and for SQL built at run time, which the host's SQL guard
//!   cannot see. The platform's own changes keep it by setting `installed`
//!   false first.
//! - **`oxy_changes_drop`** (`sql_drop`) records a captured table's drop
//!   rather than refusing it: a truncate marker, so the table's current state
//!   empties at it, then its `tables` row goes. Its `shapes` stay, for the
//!   history already shipped.
//!
//! Both functions are `SECURITY DEFINER` with a pinned `search_path`: they
//! fire for whichever role ran the DDL, and must read `oxy_changes`, which no
//! writer may. `EXECUTE` is revoked from `PUBLIC`, as for every function in
//! an Oxy schema (the grant audit refuses one `PUBLIC` holds); the event
//! triggers call them regardless.
//!
//! **Only a role that may create event triggers gets them.** On Neon the
//! owner may, through `neon_superuser`. On the local provider the owner is a
//! plain `CREATEROLE` role and may not, so the creation is skipped on
//! `insufficient_privilege` and the step still succeeds: capture works
//! without the guard, and the relay's drift check (design §5.2 layer 3) is
//! what holds there.

use crate::platform::{
    CAPTURE_FUNCTION_PREFIX, CHANGES_DROP_TRIGGER, CHANGES_GUARD_TRIGGER, CHANGES_LOG_TABLE,
    CHANGES_SCHEMA, CHANGES_STREAM_TABLE, CHANGES_TABLES_TABLE,
};

/// The command tags after which the guard checks every installed table.
const GUARDED_TAGS: [&str; 6] = [
    "ALTER TABLE",
    "CREATE TRIGGER",
    "ALTER TRIGGER",
    "DROP TRIGGER",
    "DROP TABLE",
    "DROP OWNED",
];

/// The two functions, each with `EXECUTE` revoked from `PUBLIC`, then the
/// block creating whichever event trigger is missing, where permitted.
///
/// Step 8 reaches every store, flag or not, so both functions return at once
/// when the store captures no table: an org that never opted in runs one
/// `EXISTS` per guarded statement and nothing else of this module.
pub(super) fn guard_statements() -> Vec<String> {
    vec![
        guard_ddl_function(),
        format!("REVOKE ALL ON FUNCTION {CHANGES_SCHEMA}.guard_ddl() FROM PUBLIC"),
        record_drop_function(),
        format!("REVOKE ALL ON FUNCTION {CHANGES_SCHEMA}.record_drop() FROM PUBLIC"),
        changes_guard_sql(),
    ]
}

/// `oxy_changes.guard_ddl()`. The trigger each capture function needs is
/// spelled out by its event (`tgtype`: 4 insert, 16 update, 8 delete, 32
/// truncate, each `AFTER … FOR EACH STATEMENT`) and its transition tables, so
/// a trigger replaced with a `WHEN` clause, another event, or without the
/// transition table the function reads counts as missing.
fn guard_ddl_function() -> String {
    let prefix = CAPTURE_FUNCTION_PREFIX;
    let in_place = capture_triggers_in_place("t.relid", "t.schema_name");
    format!(
        "CREATE OR REPLACE FUNCTION {CHANGES_SCHEMA}.guard_ddl() RETURNS event_trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, pg_temp AS $$
DECLARE
  v_table text;
BEGIN
  IF NOT EXISTS (SELECT 1 FROM {CHANGES_TABLES_TABLE}) THEN
    RETURN;
  END IF;
  SELECT format('%I.%I', t.schema_name, t.table_name) INTO v_table
    FROM {CHANGES_TABLES_TABLE} t
    JOIN pg_catalog.pg_class c ON c.oid = t.relid
    JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
   WHERE t.installed
     AND (c.relname <> t.table_name
          OR n.nspname <> t.schema_name
          OR {in_place} < 4)
   ORDER BY 1
   LIMIT 1;
  IF v_table IS NOT NULL THEN
    RAISE EXCEPTION 'oxy_changes: % is captured, so its four capture triggers, its name and its schema must stay as installed; remove it from oltpChanges and publish first', v_table
      USING ERRCODE = 'insufficient_privilege';
  END IF;
  SELECT format('%s calls %I.%I', g.tgrelid::regclass, pn.nspname, p.proname) INTO v_table
    FROM pg_catalog.pg_trigger g
    JOIN pg_catalog.pg_proc p ON p.oid = g.tgfoid
    JOIN pg_catalog.pg_namespace pn ON pn.oid = p.pronamespace
   WHERE pn.nspname LIKE 'app\\_%'
     AND p.proname ~ '^{prefix}[0-9]+_[iudt]$'
     AND substring(p.proname FROM '^{prefix}([0-9]+)_[iudt]$') <> g.tgrelid::text
   ORDER BY 1
   LIMIT 1;
  IF v_table IS NOT NULL THEN
    RAISE EXCEPTION 'oxy_changes: a trigger on % generated for another table; a capture function records only the table it was generated for, so drop that trigger', v_table
      USING ERRCODE = 'insufficient_privilege';
  END IF;
END
$$"
    )
}

/// How many of a table's four capture triggers are in place, as one scalar
/// subquery: each `ENABLE ALWAYS`, unfiltered, on its own event with its own
/// transition tables, and calling the function generated for that table in
/// `schema`. `relid` is an `oid` expression, `schema` a `text` one, both
/// spliced as written. The guard refuses DDL after which it is below 4; the
/// relay's drift check (`crate::changes::drift_sql`) reports a table that is.
pub(crate) fn capture_triggers_in_place(relid: &str, schema: &str) -> String {
    let prefix = CAPTURE_FUNCTION_PREFIX;
    format!(
        "(SELECT count(*)
                FROM (VALUES ('i', 4, NULL, 'oxy_new'), ('u', 16, 'oxy_old', 'oxy_new'),
                             ('d', 8, 'oxy_old', NULL), ('t', 32, NULL, NULL))
                     AS w (op, tgtype, old_table, new_table)
               WHERE EXISTS (
                 SELECT 1
                   FROM pg_catalog.pg_trigger g
                   JOIN pg_catalog.pg_proc p ON p.oid = g.tgfoid
                   JOIN pg_catalog.pg_namespace pn ON pn.oid = p.pronamespace
                  WHERE g.tgrelid = {relid}
                    AND g.tgenabled = 'A'
                    AND g.tgtype = w.tgtype
                    AND g.tgqual IS NULL
                    AND g.tgoldtable::text IS NOT DISTINCT FROM w.old_table
                    AND g.tgnewtable::text IS NOT DISTINCT FROM w.new_table
                    AND pn.nspname = {schema}
                    AND p.proname = '{prefix}' || {relid}::text || '_' || w.op))"
    )
}

/// `oxy_changes.record_drop()`: a `t` marker for each dropped table this
/// store captures (only while the stream captures), then its `tables` row
/// goes. `shapes` is not touched.
fn record_drop_function() -> String {
    format!(
        "CREATE OR REPLACE FUNCTION {CHANGES_SCHEMA}.record_drop() RETURNS event_trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, pg_temp AS $$
DECLARE
  v_dropped record;
BEGIN
  IF NOT EXISTS (SELECT 1 FROM {CHANGES_TABLES_TABLE}) THEN
    RETURN;
  END IF;
  FOR v_dropped IN
    SELECT t.relid, t.writer, t.table_name, t.schema_version
      FROM pg_catalog.pg_event_trigger_dropped_objects() d
      JOIN {CHANGES_TABLES_TABLE} t ON t.relid = d.objid
     WHERE d.object_type = 'table'
  LOOP
    INSERT INTO {CHANGES_LOG_TABLE} (generation, writer, table_name, schema_version, op)
    SELECT s.generation, v_dropped.writer, v_dropped.table_name, v_dropped.schema_version, 't'::\"char\"
      FROM {CHANGES_STREAM_TABLE} s
     WHERE s.capturing;
    DELETE FROM {CHANGES_TABLES_TABLE} WHERE relid = v_dropped.relid;
  END LOOP;
END
$$"
    )
}

/// One block, so the two triggers are created together or not at all: each
/// only when `pg_event_trigger` lacks it, and neither where the owner may
/// not create event triggers.
///
/// Any other failure is a warning too, never an error. Step 8 runs in the
/// same reconcile as the events outbox and the inbox, so a provider that
/// refused event triggers in some new way would otherwise stall every store's
/// platform schema. The guard is one layer of three: without it capture still
/// works, and the relay's drift check reports a store that has none.
pub fn changes_guard_sql() -> String {
    let create = create_missing_triggers();
    format!(
        "DO $$
BEGIN
{create}EXCEPTION WHEN insufficient_privilege THEN
  NULL;
WHEN OTHERS THEN
  RAISE WARNING 'oxy_changes: the DDL guard was not created (%): %', SQLSTATE, SQLERRM;
END
$$"
    )
}

/// What the relay runs, as the store's owner, on a store whose guard is
/// missing or disabled: [`changes_guard_sql`]'s creation of whichever event
/// trigger is missing, then the guard enabled again if it is disabled (`D`,
/// or `R`, which fires only in replica sessions), as it was created. Nothing
/// is caught: an owner that may not create or alter event triggers fails it
/// with `insufficient_privilege`, which the relay records, and any other
/// failure is the relay's to log and retry.
pub fn repair_guard_sql() -> String {
    let create = create_missing_triggers();
    format!(
        "DO $$
BEGIN
{create}  IF EXISTS (SELECT 1 FROM pg_catalog.pg_event_trigger
              WHERE evtname = '{CHANGES_GUARD_TRIGGER}' AND evtenabled NOT IN ('O', 'A')) THEN
    ALTER EVENT TRIGGER {CHANGES_GUARD_TRIGGER} ENABLE;
  END IF;
END
$$"
    )
}

/// The `IF NOT EXISTS … CREATE EVENT TRIGGER` of each of the two, as lines of
/// a `DO` block's body.
fn create_missing_triggers() -> String {
    let tags = GUARDED_TAGS
        .iter()
        .map(|tag| format!("'{tag}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "  IF NOT EXISTS (SELECT 1 FROM pg_catalog.pg_event_trigger WHERE evtname = '{CHANGES_GUARD_TRIGGER}') THEN
    CREATE EVENT TRIGGER {CHANGES_GUARD_TRIGGER} ON ddl_command_end
      WHEN TAG IN ({tags})
      EXECUTE FUNCTION {CHANGES_SCHEMA}.guard_ddl();
  END IF;
  IF NOT EXISTS (SELECT 1 FROM pg_catalog.pg_event_trigger WHERE evtname = '{CHANGES_DROP_TRIGGER}') THEN
    CREATE EVENT TRIGGER {CHANGES_DROP_TRIGGER} ON sql_drop
      EXECUTE FUNCTION {CHANGES_SCHEMA}.record_drop();
  END IF;
"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_names_every_tag() {
        let block = changes_guard_sql();
        assert!(
            block.contains(
                "WHEN TAG IN ('ALTER TABLE', 'CREATE TRIGGER', 'ALTER TRIGGER', 'DROP TRIGGER', \
                 'DROP TABLE', 'DROP OWNED')"
            ),
            "{block}"
        );
        assert!(
            block.contains("CREATE EVENT TRIGGER oxy_changes_guard ON ddl_command_end"),
            "{block}"
        );
        assert!(
            block.contains("EXECUTE FUNCTION oxy_changes.guard_ddl()"),
            "{block}"
        );
        assert!(
            block.contains("CREATE EVENT TRIGGER oxy_changes_drop ON sql_drop"),
            "{block}"
        );
        assert!(
            block.contains("EXECUTE FUNCTION oxy_changes.record_drop()"),
            "{block}"
        );
    }

    /// Both fire for whichever role ran the DDL and must read `oxy_changes`,
    /// which no writer may; neither is callable by `PUBLIC`.
    #[test]
    fn guard_runs_as_definer_with_a_fixed_path() {
        let statements = guard_statements();
        for function in [&statements[0], &statements[2]] {
            assert!(
                function.contains(
                    "RETURNS event_trigger\nLANGUAGE plpgsql SECURITY DEFINER \
                     SET search_path = pg_catalog, pg_temp AS $$"
                ),
                "{function}"
            );
        }
        assert_eq!(
            statements[1],
            "REVOKE ALL ON FUNCTION oxy_changes.guard_ddl() FROM PUBLIC"
        );
        assert_eq!(
            statements[3],
            "REVOKE ALL ON FUNCTION oxy_changes.record_drop() FROM PUBLIC"
        );
    }

    #[test]
    fn event_triggers_are_created_only_when_absent_and_permitted() {
        let block = changes_guard_sql();
        for name in [CHANGES_GUARD_TRIGGER, CHANGES_DROP_TRIGGER] {
            assert!(
                block.contains(&format!(
                    "IF NOT EXISTS (SELECT 1 FROM pg_catalog.pg_event_trigger WHERE evtname = \
                     '{name}') THEN"
                )),
                "{name}: {block}"
            );
        }
        assert!(
            block.contains("EXCEPTION WHEN insufficient_privilege THEN\n  NULL;\n"),
            "{block}"
        );
        assert!(
            block.contains("WHEN OTHERS THEN\n  RAISE WARNING"),
            "no other failure may fail the step: {block}"
        );
        assert!(block.starts_with("DO $$"), "{block}");
        assert!(
            !block[5..block.len() - 2].contains("$$"),
            "the body must not close the quote: {block}"
        );
    }

    /// The relay's repair creates what is missing, enables a disabled guard,
    /// and catches nothing, so it can tell a refusal from any other failure.
    #[test]
    fn the_relays_guard_repair_enables_the_guard_and_catches_nothing() {
        let block = repair_guard_sql();
        assert!(
            block.contains("CREATE EVENT TRIGGER oxy_changes_guard ON ddl_command_end"),
            "{block}"
        );
        assert!(
            block.contains("CREATE EVENT TRIGGER oxy_changes_drop ON sql_drop"),
            "{block}"
        );
        assert!(
            block.contains(
                "evtname = 'oxy_changes_guard' AND evtenabled NOT IN ('O', 'A')) THEN\n    \
                 ALTER EVENT TRIGGER oxy_changes_guard ENABLE;"
            ),
            "{block}"
        );
        assert!(!block.contains("EXCEPTION"), "{block}");
        assert!(block.starts_with("DO $$"), "{block}");
        assert!(
            !block[5..block.len() - 2].contains("$$"),
            "the body must not close the quote: {block}"
        );
    }

    /// The guard's text is today's, byte for byte, with the trigger count
    /// drawn from [`capture_triggers_in_place`].
    #[test]
    fn the_guard_body_is_unchanged() {
        assert_eq!(
            guard_ddl_function(),
            include_str!("guard_ddl.golden.sql").trim_end()
        );
    }

    /// An installed table keeps four capture triggers, each enabled always,
    /// calling its own functions; the error says what to do instead.
    #[test]
    fn the_guard_checks_each_installed_tables_triggers_and_name() {
        let body = guard_ddl_function();
        for needle in [
            "WHERE t.installed",
            "c.relname <> t.table_name",
            "n.nspname <> t.schema_name",
            "g.tgenabled = 'A'",
            "g.tgqual IS NULL",
            "p.proname = '_oxy_capture_' || t.relid::text || '_' || w.op)) < 4)",
            "remove it from oltpChanges and publish first",
            "USING ERRCODE = 'insufficient_privilege'",
            r"WHERE pn.nspname LIKE 'app\_%'",
            "substring(p.proname FROM '^_oxy_capture_([0-9]+)_[iudt]$') <> g.tgrelid::text",
        ] {
            assert!(body.contains(needle), "missing `{needle}`: {body}");
        }
    }

    /// A drop is recorded, not refused: a marker while the stream captures,
    /// then the `tables` row goes and the shapes stay.
    #[test]
    fn a_dropped_table_leaves_a_marker_and_its_shapes() {
        let body = record_drop_function();
        assert!(
            body.contains("FROM pg_catalog.pg_event_trigger_dropped_objects() d"),
            "{body}"
        );
        assert!(body.contains("WHERE d.object_type = 'table'"), "{body}");
        assert!(body.contains("'t'::\"char\""), "{body}");
        assert!(body.contains("WHERE s.capturing;"), "{body}");
        assert!(
            body.contains("DELETE FROM oxy_changes.tables WHERE relid = v_dropped.relid;"),
            "{body}"
        );
        assert!(!body.contains("shapes"), "{body}");
    }
}
