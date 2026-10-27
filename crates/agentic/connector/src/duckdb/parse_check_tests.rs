use std::path::Path;

use super::*;
use crate::duckdb::LoadStrategy;

fn shadow() -> Shadow {
    let orders = TableInfo {
        name: "orders".into(),
        columns: vec![
            ("order_id".into(), "BIGINT".into()),
            ("total".into(), "DOUBLE".into()),
        ],
        source: TableSource::File {
            path: "/data/orders.csv".into(),
            strategy: LoadStrategy::View,
        },
    };
    Shadow::build(&[orders], Path::new("/nonexistent")).expect("shadow")
}

#[test]
fn queries_over_the_catalog_and_its_aliases_are_selects() {
    let s = shadow();
    let refused: Vec<&str> = [
        "SELECT * FROM orders",
        r#"SELECT count(*) FROM "orders.csv""#,
        "SELECT total FROM orders.csv",
        r#"SELECT order_id FROM "orders"."csv""#,
        "WITH t AS (SELECT 1 AS x) SELECT x FROM t",
        "FROM orders LIMIT 5",
        "SELECT $$it's$$ AS s",
        r"SELECT regexp_matches('a1', '\d+')",
        "SELECT 1; SELECT 2",
        "",
        // DuckDB rewrites these to queries, and types them so.
        "DESCRIBE orders",
        "SUMMARIZE orders",
        "SHOW TABLES",
        "PRAGMA table_info('orders')",
    ]
    .into_iter()
    .filter(|sql| !s.only_selects(sql))
    .collect();
    assert!(refused.is_empty(), "queries refused: {refused:#?}");
}

/// DuckDB's parser is the one that runs the text, so what it reads as a
/// statement other than a query — however the text hides it — is refused.
#[test]
fn every_statement_that_is_not_a_query_is_refused() {
    let s = shadow();
    let passed: Vec<&str> = [
        "CREATE TABLE x AS SELECT 1",
        "CREATE OR REPLACE VIEW orders AS SELECT 42",
        "DROP TABLE orders",
        "INSERT INTO orders VALUES (1, 2.0)",
        "UPDATE orders SET total = 0",
        "DELETE FROM orders",
        "ALTER TABLE orders ADD COLUMN y INT",
        "COPY orders TO 'x.csv'",
        "SET threads = 1",
        "PRAGMA threads = 1",
        "ATTACH ':memory:' AS other",
        "BEGIN TRANSACTION",
        "EXPLAIN SELECT 1",
        "EXPLAIN ANALYZE SELECT 1",
        // Without an `IN` list a PIVOT first creates an enum type.
        "PIVOT orders ON order_id USING sum(total)",
        // The F1 shape: a quote inside a dollar quote.
        "WITH a AS (SELECT $$it's$$ AS s) INSERT INTO orders SELECT 1, 2.0 FROM a",
        // The F2 shape: `x$y$` is one identifier to DuckDB.
        "SELECT 1 AS x$y$; DROP TABLE orders; SELECT 1 AS x$y$",
        // Does not bind: what it would read is not here.
        "SELECT * FROM no_such_table",
        "SELECT 1 FROM",
    ]
    .into_iter()
    .filter(|sql| s.only_selects(sql))
    .collect();
    assert!(passed.is_empty(), "non-queries passed: {passed:#?}");
}

/// A query's table functions cannot carry a write either: `query()` runs
/// only a query. Checked against DuckDB itself, since the parser check
/// trusts a `SELECT` that calls it.
#[test]
fn a_query_cannot_run_ddl_through_the_query_table_function() {
    let conn = duckdb::Connection::open_in_memory().unwrap();
    let _ = conn.execute_batch("SELECT * FROM query('CREATE TABLE zz AS SELECT 1')");
    let made: i64 = conn
        .query_row(
            "SELECT count(*) FROM duckdb_tables() WHERE table_name = 'zz'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(made, 0, "query() created a table");
}

/// Times one classification against the shadow of a real directory.
/// `OXY_DUCKDB_CACHE_BENCH_DIR=<dir> cargo nextest run -p agentic-connector
/// --features duckdb --lib --run-ignored only --no-capture measure_a_parse`.
#[test]
#[ignore = "measures a real directory named by OXY_DUCKDB_CACHE_BENCH_DIR"]
fn measure_a_parse_check() {
    let Ok(dir) = std::env::var("OXY_DUCKDB_CACHE_BENCH_DIR") else {
        eprintln!("OXY_DUCKDB_CACHE_BENCH_DIR is not set; nothing to measure");
        return;
    };
    let dir = Path::new(&dir);
    let connector =
        crate::duckdb::DuckDbConnector::from_directory(dir, LoadStrategy::View).unwrap();
    let tables = connector.loaded_tables().to_vec();
    let started = std::time::Instant::now();
    let shadow = Shadow::build(&tables, dir).expect("shadow");
    eprintln!(
        "{} tables: shadow built in {:?}",
        tables.len(),
        started.elapsed()
    );
    let first = &tables[0].name;
    for sql in [
        format!(r#"SELECT * FROM "{first}" LIMIT 10"#),
        format!(r#"WITH a AS (SELECT * FROM "{first}") SELECT count(*) FROM a"#),
    ] {
        assert!(shadow.only_selects(&sql), "{sql}");
        let runs = 200;
        let started = std::time::Instant::now();
        for _ in 0..runs {
            shadow.only_selects(&sql);
        }
        eprintln!("{sql}: {:?} per check", started.elapsed() / runs);
    }
}
