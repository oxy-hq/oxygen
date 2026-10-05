//! The classifier's contract: nothing that writes ever reads as `Read`.

use std::any::TypeId;

use sqlparser::dialect::{
    BigQueryDialect, ClickHouseDialect, DuckDbDialect, GenericDialect, PostgreSqlDialect,
    SQLiteDialect, SnowflakeDialect,
};

use super::*;

fn write(verb: &str, targets: &[&str]) -> StatementKind {
    StatementKind::Write {
        verb: verb.to_string(),
        targets: targets.iter().map(|t| t.to_string()).collect(),
    }
}

fn assert_held(dialect: SqlDialect, sql: &str) -> Vec<StatementKind> {
    let kinds = classify(dialect, sql);
    assert!(!is_all_read(&kinds), "`{sql}` must be held, got {kinds:?}");
    kinds
}

/// A mutation's verb follows the table name (past an optional ON CLUSTER);
/// a `DELETE` or `UPDATE` later in an `ALTER` that does not parse is not one,
/// so the statement stays unclassified — held, and never named a mutation of
/// a table it may not touch.
#[test]
fn a_clickhouse_mutation_verb_must_follow_the_table_name() {
    let ch = SqlDialect::CLICKHOUSE;
    for sql in [
        "ALTER TABLE orders MODIFY SETTING x = 1, DELETE WHERE 1",
        "ALTER TABLE orders ON CLUSTER main MODIFY TTL d + INTERVAL 1 DAY DELETE",
        "ALTER TABLE orders FREEZE WITH NAME 'UPDATE'",
    ] {
        let kinds = assert_held(ch, sql);
        assert!(
            matches!(kinds.as_slice(), [StatementKind::Unclassified(_)]),
            "{sql}: {kinds:?}"
        );
    }
    assert_eq!(
        assert_held(
            ch,
            "ALTER TABLE db.orders ON CLUSTER 'main' DELETE WHERE id = 1"
        ),
        vec![write("ALTER TABLE DELETE", &["db.orders"])]
    );
}

#[test]
fn clickhouse_mutations_are_writes() {
    let ch = SqlDialect::CLICKHOUSE;
    assert_eq!(
        assert_held(
            ch,
            "ALTER TABLE toast.orders DELETE WHERE business_date < '2026-01-01'"
        ),
        vec![write("ALTER TABLE DELETE", &["toast.orders"])]
    );
    assert_eq!(
        assert_held(
            ch,
            "ALTER TABLE orders ON CLUSTER main UPDATE voided = 1 WHERE guid = 'g'"
        ),
        vec![write("ALTER TABLE UPDATE", &["orders"])]
    );
    assert_eq!(
        assert_held(ch, "OPTIMIZE TABLE toast.orders FINAL"),
        vec![write("OPTIMIZE", &["toast.orders"])]
    );
    assert_eq!(
        assert_held(ch, "INSERT INTO rollups.daily SELECT * FROM toast.orders"),
        vec![write("INSERT", &["rollups.daily"])]
    );
    assert_eq!(
        assert_held(ch, "TRUNCATE TABLE rollups.daily"),
        vec![write("TRUNCATE", &["rollups.daily"])]
    );
    assert_eq!(
        assert_held(ch, "RENAME TABLE a TO b"),
        vec![write("RENAME TABLE", &["a", "b"])]
    );
    assert_eq!(
        assert_held(ch, "DROP TABLE IF EXISTS rollups.daily"),
        vec![write("DROP TABLE", &["rollups.daily"])]
    );
    // Statements the parser does not know are held all the same.
    for sql in [
        "SYSTEM FLUSH LOGS",
        "EXCHANGE TABLES a AND b",
        "KILL QUERY WHERE query_id = 'x'",
    ] {
        assert!(matches!(
            assert_held(ch, sql).as_slice(),
            [StatementKind::Unclassified(_)]
        ));
    }
}

#[test]
fn select_into_is_a_write() {
    let pg = SqlDialect::Postgres;
    assert_eq!(
        assert_held(pg, "SELECT * INTO scratch.copy FROM public.orders"),
        vec![write("SELECT INTO", &["scratch.copy"])]
    );
    assert_eq!(
        assert_held(pg, "SELECT a FROM t UNION SELECT a INTO u FROM s"),
        vec![write("SELECT INTO", &["u"])]
    );
    // A data-modifying CTE is a SELECT on the outside.
    assert_eq!(
        assert_held(
            pg,
            "WITH gone AS (DELETE FROM orders RETURNING *) SELECT count(*) FROM gone"
        ),
        vec![write("DELETE", &["orders"])]
    );
    // EXPLAIN ANALYZE runs its statement.
    assert_eq!(
        assert_held(pg, "EXPLAIN ANALYZE DELETE FROM orders"),
        vec![write("DELETE", &["orders"])]
    );
}

#[test]
fn multi_statement_with_one_write_is_a_write() {
    let kinds = assert_held(
        SqlDialect::Postgres,
        "SELECT 1; DELETE FROM orders WHERE id = 1; SELECT 2",
    );
    assert_eq!(
        kinds,
        vec![
            StatementKind::Read,
            write("DELETE", &["orders"]),
            StatementKind::Read
        ]
    );
    assert_eq!(first_non_read(&kinds), Some(&write("DELETE", &["orders"])));
}

#[test]
fn unparseable_is_unclassified() {
    for sql in [
        "SELEC * FROM t",
        "",
        "   ",
        "-- only a comment",
        "/* nothing */",
    ] {
        let kinds = classify(SqlDialect::Postgres, sql);
        assert!(
            matches!(kinds.as_slice(), [StatementKind::Unclassified(_)]),
            "`{sql}` → {kinds:?}"
        );
        assert!(!is_all_read(&kinds));
    }
    assert!(!is_all_read(&[]), "no statements is never all-read");
}

/// Session and procedure statements change something this cannot see.
#[test]
fn session_and_procedure_statements_are_held() {
    for (dialect, sql, verb) in [
        (SqlDialect::Snowflake, "USE WAREHOUSE transform_wh", "USE"),
        (SqlDialect::Postgres, "SET search_path = staging", "SET"),
        (SqlDialect::Snowflake, "CALL refresh_rollups()", "CALL"),
        (
            SqlDialect::Postgres,
            "GRANT SELECT ON orders TO analyst",
            "GRANT",
        ),
    ] {
        let kinds = assert_held(dialect, sql);
        assert!(
            matches!(&kinds[..], [StatementKind::Write { verb: v, .. }] if v == verb),
            "`{sql}` → {kinds:?}"
        );
    }
}

/// The other half of the contract: a fence that held every read would be
/// useless, and would teach people to turn it off.
#[test]
fn reads_are_reads() {
    for (dialect, sql) in [
        (
            SqlDialect::CLICKHOUSE,
            "SELECT * FROM toast.orders FINAL WHERE business_date = '2026-09-27'",
        ),
        (
            SqlDialect::CLICKHOUSE,
            "SELECT a, arrayJoin(b) FROM db.t SETTINGS max_threads = 1",
        ),
        (
            SqlDialect::CLICKHOUSE,
            "SELECT toStartOfDay(ts) AS d, count() FROM t GROUP BY d ORDER BY d",
        ),
        (SqlDialect::CLICKHOUSE, "SELECT * FROM t LIMIT 1 BY a"),
        (
            SqlDialect::CLICKHOUSE,
            "WITH x AS (SELECT 1) SELECT * FROM x",
        ),
        (SqlDialect::CLICKHOUSE, "SHOW TABLES"),
        (SqlDialect::CLICKHOUSE, "DESCRIBE TABLE toast.orders"),
        (SqlDialect::CLICKHOUSE, "EXPLAIN SELECT 1"),
        (SqlDialect::Postgres, "SELECT * FROM orders FOR UPDATE"),
        (
            SqlDialect::Snowflake,
            "SELECT * FROM db.s.t QUALIFY row_number() OVER (PARTITION BY a ORDER BY b) = 1",
        ),
        (SqlDialect::BigQuery, "SELECT * FROM `proj.ds.t`"),
        (
            SqlDialect::DuckDb,
            "SELECT * FROM toast_pos.sales_daily_metrics",
        ),
        (SqlDialect::Sqlite, "SELECT 1"),
        (SqlDialect::Other("MySQL"), "SELECT 1; SELECT 2"),
    ] {
        let kinds = classify(dialect, sql);
        assert!(
            is_all_read(&kinds),
            "`{sql}` must read as a read: {kinds:?}"
        );
    }
}

#[test]
fn each_connector_dialect_parses_as_itself() {
    for (dialect, expected) in [
        (SqlDialect::CLICKHOUSE, TypeId::of::<ClickHouseDialect>()),
        (SqlDialect::Snowflake, TypeId::of::<SnowflakeDialect>()),
        (SqlDialect::BigQuery, TypeId::of::<BigQueryDialect>()),
        (SqlDialect::Postgres, TypeId::of::<PostgreSqlDialect>()),
        (SqlDialect::DuckDb, TypeId::of::<DuckDbDialect>()),
        (SqlDialect::Sqlite, TypeId::of::<SQLiteDialect>()),
        (SqlDialect::Other("MySQL"), TypeId::of::<GenericDialect>()),
    ] {
        assert_eq!(parser_dialect(dialect).dialect(), expected, "{dialect:?}");
    }
}

/// Run `f` on a thread with a Tokio worker's 2 MiB stack, as production does.
fn on_a_tokio_sized_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(f)
        .expect("spawn a 2 MiB thread")
        .join()
        .expect("classifying must never abort a 2 MiB caller")
}

/// A staging `ctx.oltp.query` of ~30,000 OR terms overflowed a Tokio
/// worker's stack when `classify` parsed on it, aborting the pod. Now SQL
/// past the depth bound is held unparsed, and deep SQL under it is
/// classified on the big stack — a 30,000-link chain overflows a 2 MiB stack
/// (it holds ~22,000) — from a 2 MiB caller either way.
#[test]
fn a_deep_or_chain_is_held_or_classified_never_aborts() {
    let where_or = |term: fn(usize) -> String, terms: usize| {
        let terms = (0..terms).map(term).collect::<Vec<_>>();
        format!("SELECT * FROM orders WHERE {}", terms.join(" OR "))
    };
    let held = where_or(|i| format!("id = {i}"), 50_000);
    let held = on_a_tokio_sized_stack(move || classify(SqlDialect::Postgres, &held));
    assert!(
        matches!(held.as_slice(), [StatementKind::Unclassified(why)] if why.contains("nests")),
        "{held:?}"
    );
    let deep = where_or(|_| "flag".to_string(), 30_000);
    let deep = on_a_tokio_sized_stack(move || classify(SqlDialect::Postgres, &deep));
    assert_eq!(deep, vec![StatementKind::Read]);
}

/// `set_config` is how a read would switch a read-only Postgres session back
/// (`hold::pg`), so a call to it is a write in any schema or casing, as a
/// scalar or in `FROM`, and so is a built-in that runs SQL text unseen.
#[test]
fn set_config_is_a_write_however_it_is_spelled() {
    for sql in [
        "SELECT set_config('default_transaction_read_only', 'off', false)",
        "SELECT PG_CATALOG.SET_CONFIG('a', 'b', true) AS x",
        "select \"pg_catalog\".\"set_config\"('a', 'b', true)",
        "SELECT * FROM set_config('a', 'b', false)",
        "SELECT 1 FROM t WHERE (SELECT set_config('a', 'b', false)) IS NOT NULL",
        "WITH s AS (SELECT set_config('a', 'b', false)) SELECT * FROM s",
        "SELECT query_to_xml('select set_config(''a'', ''b'', false)', true, false, '')",
        "SELECT * FROM ts_stat('select 1')",
    ] {
        assert_eq!(
            classify(SqlDialect::Postgres, sql),
            vec![write("SET_CONFIG", &[])],
            "{sql}"
        );
    }
    for sql in [
        "SELECT current_setting('default_transaction_read_only')",
        "SELECT 'set_config(' AS note",
        "SELECT set_configuration FROM t",
    ] {
        assert_eq!(
            classify(SqlDialect::Postgres, sql),
            vec![StatementKind::Read],
            "{sql}"
        );
    }
}
