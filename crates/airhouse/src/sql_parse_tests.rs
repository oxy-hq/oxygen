use sqlparser::dialect::GenericDialect;

use super::{MAX_SQL_DEPTH, on_sql_stack, with_parsed};
use crate::sql_rules::{Access, Refused, check};

/// The stack a Tokio worker has, and Rust's default for a spawned thread.
const TOKIO_STACK: usize = 2 << 20;

/// Run `f` on a thread with a Tokio worker's stack, as production does.
fn on_a_tokio_sized_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(TOKIO_STACK)
        .spawn(f)
        .expect("spawn a 2 MiB thread")
        .join()
        .expect("a 2 MiB caller must never abort")
}

fn chain(terms: usize) -> String {
    format!("SELECT {}", vec!["1"; terms].join("+"))
}

#[test]
fn app_reads_of_names_duckdb_reads_as_files_are_refused() {
    for sql in [
        "SELECT * FROM 's3://lake-bucket/other_tenant/x.parquet'",
        "SELECT * FROM '/etc/passwd'",
        "SELECT * FROM 'visits'",
        "SELECT * FROM \"x.parquet\"",
        "SELECT * FROM x.parquet",
        "SELECT * FROM app_store_ops.csv",
        "DESCRIBE 's3://lake-bucket/x.json'",
    ] {
        let msg = check(sql, "app_store_ops", Access::Read).unwrap_err().0;
        assert!(msg.contains("looks like a file"), "{sql}: {msg}");
    }
    check(
        "SELECT * FROM app_store_ops.visits",
        "app_store_ops",
        Access::Read,
    )
    .unwrap();
}

#[test]
fn app_writes_to_tables_named_like_file_types_pass() {
    // `ctx.airhouse.append({table: "json"})` sends the first; a migration
    // may create such a table. DuckDB reads files only in read positions.
    for (sql, access) in [
        (r#"INSERT INTO "app_x"."json" VALUES (1)"#, Access::Write),
        ("UPDATE app_x.json SET a = 1", Access::Write),
        ("DELETE FROM app_x.parquet WHERE a = 1", Access::Write),
        ("CREATE TABLE app_x.csv (id VARCHAR)", Access::Ddl),
        ("ALTER TABLE app_x.csv ADD COLUMN note VARCHAR", Access::Ddl),
        ("DROP TABLE app_x.csv", Access::Ddl),
    ] {
        if let Err(e) = check(sql, "app_x", access) {
            panic!("expected {sql:?} to pass as {access:?}: {e}");
        }
    }
    for sql in [
        "SELECT * FROM x.parquet",
        "DELETE FROM app_x.t USING x.parquet WHERE true",
        "UPDATE app_x.t SET a = 1 FROM app_x.json j WHERE t.id = j.id",
    ] {
        let msg = check(sql, "app_x", Access::Write).unwrap_err().0;
        assert!(msg.contains("looks like a file"), "{sql}: {msg}");
    }
}

#[test]
fn apps_may_not_advance_a_sequence() {
    let msg = check("SELECT nextval('s')", "app_store_ops", Access::Read)
        .unwrap_err()
        .0;
    assert!(msg.contains("sequence"), "{msg}");
}

/// The thread `on_sql_stack` ran `work` on, called from `caller`'s stack.
fn ran_on(sql: String) -> Option<String> {
    on_a_tokio_sized_stack(move || {
        on_sql_stack(&sql, "an app", |_| {
            Ok::<_, Refused>(std::thread::current().name().map(str::to_string))
        })
        .expect("parses")
    })
}

/// What `ctx.airhouse.append` builds (`host::columns_and_values`) for
/// `rows` rows of ten columns, every literal kind it renders included.
fn append_insert(rows: usize) -> String {
    let columns = (0..10).map(|c| format!("\"c{c}\"")).collect::<Vec<_>>();
    let row = |i: usize| {
        format!(
            "('visit-{i}', {i}, -{i}.5, NULL, true, false, 'it''s', '{{\"k\":[1,2]}}', \
             '2024-01-01T00:00:00Z', 0)"
        )
    };
    format!(
        r#"INSERT INTO "app_x"."visits" ({}) VALUES {}"#,
        columns.join(", "),
        (0..rows).map(row).collect::<Vec<_>>().join(", ")
    )
}

/// A migration file past 2 MiB: tables, seed rows, alters, comments.
fn big_migration() -> String {
    let mut sql = String::new();
    let mut t = 0;
    while sql.len() < 2 << 20 {
        sql.push_str(&format!(
            "-- table {t}\nCREATE TABLE app_x.t{t} (id VARCHAR NOT NULL, amount DOUBLE, \
             at TIMESTAMP, note VARCHAR DEFAULT 'none');\n\
             ALTER TABLE app_x.t{t} ADD COLUMN extra INTEGER;\n\
             INSERT INTO app_x.t{t} VALUES {};\n",
            (0..40)
                .map(|i| format!("('r{i}', {i}.25, TIMESTAMP '2024-01-01', 'seed', {i})"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        t += 1;
    }
    sql
}

/// Size is not depth: an append of any row count and a migration of any
/// length pass exactly as they did with no guard — on the caller's own
/// thread, from a Tokio worker's stack.
#[test]
fn a_20000_row_append_and_a_2mib_migration_pass_on_the_callers_stack() {
    let append = append_insert(20_000);
    let migration = big_migration();
    assert!(append.len() > 1 << 20, "{} bytes", append.len());
    for (sql, access) in [(append, Access::Write), (migration, Access::Ddl)] {
        let checked = {
            let sql = sql.clone();
            on_a_tokio_sized_stack(move || check(&sql, "app_x", access))
        };
        if let Err(e) = checked {
            panic!("{access:?} of {} bytes refused: {e}", sql.len());
        }
        assert_ne!(ran_on(sql).as_deref(), Some("airhouse-sql"));
    }
}

/// A chain too deep for a Tokio worker's stack (it overflows one past
/// ~22,000 links) is checked on the big stack, from a 2 MiB caller.
#[test]
fn a_chain_too_deep_for_a_tokio_stack_is_checked_on_the_big_stack() {
    let sql = chain(30_000);
    let checked = {
        let sql = sql.clone();
        on_a_tokio_sized_stack(move || check(&sql, "app_x", Access::Read))
    };
    assert_eq!(checked.expect("30,000 links is checkable").len(), 1);
    assert_eq!(ran_on(sql).as_deref(), Some("airhouse-sql"));
}

/// Nested types recurse in the parser itself, with no limit: 63 levels of
/// `STRUCT(…)` overflow a 2 MiB stack. Deeper than that, from one, is
/// checked on the big stack; past the bound, refused — never an abort.
#[test]
fn a_deeply_nested_type_is_checked_on_the_big_stack_or_refused() {
    let nested = |levels: usize| {
        let ty = (0..levels).fold("INT".to_string(), |ty, _| format!("STRUCT(a {ty})"));
        format!("SELECT CAST(NULL AS {ty})")
    };
    for (sql, passes) in [
        (nested(100), true),
        (
            format!("SELECT CAST(NULL AS INT{})", "[]".repeat(100)),
            true,
        ),
        (nested(1_000), false),
    ] {
        let checked = on_a_tokio_sized_stack(move || check(&sql, "app_x", Access::Read));
        assert_eq!(checked.is_ok(), passes, "{checked:?}");
    }
}

/// A million-term chain is refused, unparsed, with a message that says why —
/// and a 2 MiB caller survives it.
#[test]
fn a_million_term_chain_is_refused_cleanly() {
    let refused = on_a_tokio_sized_stack(|| check(&chain(1_000_000), "app_x", Access::Read))
        .expect_err("refused")
        .0;
    assert!(
        refused.contains("nests") && refused.contains("split"),
        "{refused}"
    );
}

/// A few dozen tokens — a typical `ctx.airhouse.append` — must not pay the
/// thread spawn.
#[test]
fn shallow_sql_runs_on_the_callers_own_thread() {
    let caller = std::thread::current().name().map(str::to_string);
    let ran_on = on_sql_stack("SELECT * FROM app_x.t WHERE a = 1", "an app", |_| {
        Ok::<_, Refused>(std::thread::current().name().map(str::to_string))
    })
    .unwrap();
    assert_eq!(ran_on, caller);
}

#[test]
fn with_parsed_hands_work_the_parse_error_and_refuses_over_deep_sql_unparsed() {
    let unterminated = with_parsed(&GenericDialect {}, "SELECT 'x", |parsed| parsed.is_err());
    assert_eq!(unterminated, Ok(true));
    let parsed = with_parsed(&GenericDialect {}, "SELECT 1; SELECT 2", |parsed| {
        parsed.map(|statements| statements.len())
    });
    assert_eq!(parsed, Ok(Ok(2)));
    // One `OR` more than the bound.
    let deep = format!("SELECT {}", vec!["a"; MAX_SQL_DEPTH + 2].join(" OR "));
    let refused = with_parsed(&GenericDialect {}, &deep, |_| panic!("must not parse"));
    assert!(refused.is_err());
}
