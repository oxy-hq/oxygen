//! Tests for the read-only classifier.

use super::*;

#[test]
fn queries_are_read_only() {
    for sql in [
        "SELECT * FROM orders",
        "select count(*) from orders.csv",
        r#"SELECT "Date" FROM "oxymart.csv" WHERE "Date" IS NOT NULL LIMIT 20"#,
        "WITH t AS (SELECT 1 AS x) SELECT x FROM t",
        "FROM orders LIMIT 5",
        "(SELECT 1) UNION ALL (SELECT 2)",
        "DESCRIBE orders",
        "SUMMARIZE SELECT * FROM orders",
        "SHOW TABLES",
        "EXPLAIN SELECT 1",
        "EXPLAIN ANALYZE SELECT 1",
        "-- revenue\nselect 42 as answer",
        "SELECT 1; SELECT 2;",
        "",
        "  ;  ",
        "SELECT created_at, updated_by, deleted FROM t ORDER BY 1 DESC",
        // A backslash is an ordinary character in a standard string.
        "SELECT regexp_matches(name, '\\d+') FROM t",
    ] {
        assert!(is_read_only(sql), "{sql:?} reads only");
    }
}

#[test]
fn writes_are_not_read_only() {
    for sql in [
        "CREATE TABLE summary AS SELECT 1",
        "CREATE OR REPLACE VIEW orders AS SELECT 42",
        "CREATE TEMP TABLE t AS SELECT 1",
        "DROP VIEW orders",
        "INSERT INTO t VALUES (1)",
        "UPDATE t SET x = 1",
        "DELETE FROM t",
        "ALTER TABLE t ADD COLUMN y INT",
        "COPY orders TO 'out.csv'",
        "SET threads = 1",
        "PRAGMA threads = 1",
        "CALL dbgen(sf = 0.01)",
        "ATTACH ':memory:' AS other",
        "INSTALL httpfs",
        "LOAD httpfs",
        "BEGIN TRANSACTION",
        "USE other",
        "WITH t AS (SELECT 1) INSERT INTO x SELECT * FROM t",
        "EXPLAIN ANALYZE CREATE TABLE x AS SELECT 1",
        "SELECT 1; CREATE TABLE x (a INT); SELECT 2",
    ] {
        assert!(!is_read_only(sql), "{sql:?} may write");
    }
}

/// A write word inside a literal, a quoted identifier or a comment is
/// not a statement.
#[test]
fn quoted_and_commented_words_are_ignored() {
    assert!(is_read_only("SELECT 'DROP TABLE x' AS s"));
    assert!(is_read_only(r#"SELECT "insert" FROM t"#));
    assert!(is_read_only("SELECT 1 -- then DROP everything"));
    assert!(is_read_only("SELECT /* CREATE */ 1"));
    assert!(is_read_only("SELECT 'it''s; DROP TABLE x' AS s"));
}

/// A dollar-quoted string is skipped as the statement splitter skips it,
/// so a quote inside one does not hide the rest of the statement.
#[test]
fn a_dollar_quoted_string_cannot_hide_a_write() {
    let hidden: Vec<&str> = [
        "WITH a AS (SELECT $$it's$$ AS s) INSERT INTO t SELECT s FROM a",
        "WITH a AS (SELECT $q$it's$q$ AS s) DELETE FROM t",
        "WITH a AS (SELECT $$a \"b$$ AS s) UPDATE t SET x = 1",
    ]
    .into_iter()
    .filter(|sql| is_read_only(sql))
    .collect();
    assert!(hidden.is_empty(), "writes read as read-only: {hidden:#?}");
    let refused: Vec<&str> = [
        "SELECT $$it's fine$$ AS s",
        "SELECT $q$DROP TABLE x$q$ AS s",
        "SELECT $$a$$ AS s; SELECT $t$b$t$ AS t",
    ]
    .into_iter()
    .filter(|sql| !is_read_only(sql))
    .collect();
    assert!(refused.is_empty(), "reads refused: {refused:#?}");
}

/// A `SELECT` that calls an instance-level table function changes the
/// database every clone shares, however the call is spelled.
#[test]
fn instance_function_calls_are_writes() {
    let read = [
        "SELECT * FROM enable_logging()",
        "select * from ENABLE_LOGGING ( )",
        "FROM enable_logging()",
        "SELECT * FROM system.enable_logging()",
        r#"SELECT * FROM "enable_logging"()"#,
        r#"SELECT * FROM "system"."ENABLE_LOGGING"()"#,
        "SELECT * FROM `enable_logging`()",
        "SELECT * FROM enable_logging/* a comment */()",
        "SELECT * FROM enable_logging -- a comment\n()",
        "WITH t AS (SELECT * FROM enable_logging()) SELECT 1",
        "SELECT 1; SELECT * FROM enable_logging()",
        "SELECT * FROM disable_logging()",
        "SELECT * FROM enable_profiling()",
        "SELECT * FROM truncate_duckdb_logs()",
        "SELECT * FROM checkpoint()",
        "SELECT * FROM force_checkpoint()",
        // `query` runs a string as SQL, so it can build any name above.
        "SELECT * FROM query('SELECT * FROM enable_' || 'logging()')",
        "SELECT * FROM json_execute_serialized_sql('{}')",
    ]
    .into_iter()
    .filter(|sql| is_read_only(sql))
    .collect::<Vec<_>>();
    assert!(read.is_empty(), "read as read-only: {read:#?}");
}

/// The names are only a write when called: a column, a literal or a
/// comment that says them still reads.
#[test]
fn instance_function_names_that_are_not_calls_still_read() {
    let refused = [
        "SELECT query FROM searches",
        r#"SELECT "query", "checkpoint" FROM t"#,
        "SELECT 'enable_logging()' AS s",
        "SELECT 1 -- enable_logging()",
        "SELECT * FROM duckdb_logs",
        "SELECT count(*) AS query FROM t",
    ]
    .into_iter()
    .filter(|sql| !is_read_only(sql))
    .collect::<Vec<_>>();
    assert!(refused.is_empty(), "reads refused: {refused:#?}");
}

/// Table functions of the bundled DuckDB that only read: their output
/// is a function of their arguments, the files they name and the
/// catalog. Reviewed against DuckDB's `duckdb_functions()`.
const READING_FUNCTIONS: &[&str] = &[
    "arrow_scan",
    "arrow_scan_dumb",
    "duckdb_approx_database_count",
    "duckdb_columns",
    "duckdb_connection_count",
    "duckdb_constraints",
    "duckdb_coordinate_systems",
    "duckdb_databases",
    "duckdb_dependencies",
    "duckdb_extensions",
    "duckdb_external_file_cache",
    "duckdb_functions",
    "duckdb_indexes",
    "duckdb_keywords",
    "duckdb_log_contexts",
    "duckdb_logs",
    "duckdb_memory",
    "duckdb_optimizers",
    "duckdb_prepared_statements",
    "duckdb_schemas",
    "duckdb_secret_types",
    "duckdb_secrets",
    "duckdb_sequences",
    "duckdb_settings",
    "duckdb_table_sample",
    "duckdb_tables",
    "duckdb_temporary_files",
    "duckdb_types",
    "duckdb_variables",
    "duckdb_views",
    "generate_series",
    "glob",
    "icu_calendar_names",
    "json_each",
    "json_tree",
    "parquet_bloom_probe",
    "parquet_file_metadata",
    "parquet_kv_metadata",
    "parquet_metadata",
    "parquet_scan",
    "parquet_schema",
    "pg_timezone_names",
    "pragma_collations",
    "pragma_database_size",
    "pragma_metadata_info",
    "pragma_platform",
    "pragma_show",
    "pragma_storage_info",
    "pragma_table_info",
    "pragma_user_agent",
    "pragma_version",
    "query_table",
    "range",
    "read_blob",
    "read_csv",
    "read_csv_auto",
    "read_duckdb",
    "read_json",
    "read_json_auto",
    "read_json_objects",
    "read_json_objects_auto",
    "read_ndjson",
    "read_ndjson_auto",
    "read_ndjson_objects",
    "read_parquet",
    "read_text",
    "repeat",
    "repeat_row",
    "seq_scan",
    "sniff_csv",
    "summary",
    "test_all_types",
    "test_vector_types",
    "unnest",
    "which_secret",
];

/// Every table function the bundled DuckDB has is either a read or in
/// [`INSTANCE_FUNCTIONS`]: a DuckDB upgrade that adds one fails here until
/// someone decides which.
#[test]
fn every_duckdb_table_function_is_classified() {
    let conn = duckdb::Connection::open_in_memory().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT function_name FROM duckdb_functions() \
             WHERE function_type = 'table' ORDER BY 1",
        )
        .unwrap();
    let names: Vec<String> = stmt
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(names.len() > 40, "the listing is load-bearing: {names:?}");
    let unclassified: Vec<&String> = names
        .iter()
        .filter(|n| {
            !READING_FUNCTIONS.contains(&n.as_str())
                && !INSTANCE_FUNCTIONS.contains(&n.to_ascii_uppercase().as_str())
        })
        .collect();
    assert!(
        unclassified.is_empty(),
        "table functions nobody has classified as a read or an instance function: \
         {unclassified:?}"
    );
}

/// What the scanner cannot fully read is treated as a write, so the run
/// moves to its private copy rather than guessing.
#[test]
fn what_the_scanner_cannot_read_is_a_write() {
    let read = [
        ("SELECT 'unclosed", "a quote still open at the end"),
        ("SELECT \"unclosed", "an identifier still open at the end"),
        ("SELECT $$unclosed", "a dollar quote still open at the end"),
        ("SELECT 1 /* unclosed", "a comment still open at the end"),
        ("SELECT E'it\\'s' AS s", "an escape string"),
        ("SELECT e'x' AS s", "an escape string, lower case"),
        (
            "SELECT x$y$ FROM t; DROP TABLE t; SELECT 1 AS x$y$",
            "a dollar inside a name, which a closing tag after it would make a quote",
        ),
        ("SELECT $1", "a parameter, not a dollar quote"),
        (
            "SELECT a$b FROM t",
            "a dollar sign the splitter may read as a quote",
        ),
    ]
    .into_iter()
    .filter(|(sql, _)| is_read_only(sql))
    .collect::<Vec<_>>();
    assert!(read.is_empty(), "read as read-only: {read:#?}");
}
