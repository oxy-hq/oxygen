//! Tests for the directory-database cache. Each test owns a [`DirCache`], so
//! none of them reads or warms the process-wide one.

use std::{
    path::Path,
    sync::{Arc, Barrier, mpsc},
    thread,
    time::{Duration, Instant, SystemTime},
};

use agentic_core::result::CellValue;

use super::*;
use crate::connector::DatabaseConnector;

fn write_csv(dir: &Path, name: &str, body: &str) {
    std::fs::write(dir.join(name), body).unwrap();
}

fn write_parquet(dir: &Path, name: &str, select: &str) {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(&format!(
        "COPY ({select}) TO '{}' (FORMAT PARQUET)",
        dir.join(name).display()
    ))
    .unwrap();
}

/// `orders.csv` (2 rows) and `stores.parquet` (3 rows).
pub(super) fn sample_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write_csv(dir.path(), "orders.csv", "order_id,total\n1,10\n2,20\n");
    write_parquet(
        dir.path(),
        "stores.parquet",
        "SELECT 1 AS store_id UNION ALL SELECT 2 UNION ALL SELECT 3",
    );
    dir
}

pub(super) async fn first_cell(c: &DuckDbConnector, sql: &str) -> CellValue {
    let res = c
        .execute_query(sql, 10)
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
    res.result.rows[0].0[0].clone()
}

async fn count(c: &DuckDbConnector, from: &str) -> f64 {
    match first_cell(c, &format!("SELECT count(*) FROM {from}")).await {
        CellValue::Number(n) => n,
        other => panic!("count(*) FROM {from} returned {other:?}"),
    }
}

/// `(name, columns)` per registered table: what a query can rely on.
fn tables(c: &DuckDbConnector) -> Vec<(String, Vec<(String, String)>)> {
    c.loaded_tables()
        .iter()
        .map(|t| (t.name.clone(), t.columns.clone()))
        .collect()
}

/// Catalog views in the `orders` schema — the `"orders"."csv"` alias.
async fn views_in_orders(c: &DuckDbConnector) -> f64 {
    count(c, "duckdb_views() WHERE schema_name = 'orders'").await
}

pub(super) fn is_shared(c: &DuckDbConnector) -> bool {
    c.conn.session().unwrap().is_shared()
}

#[tokio::test]
async fn an_unchanged_directory_is_built_once_and_serves_the_same_tables() {
    let dir = sample_dir();
    let cache = DirCache::new(4);

    let first = cache.connector(dir.path(), LoadStrategy::View).unwrap();
    let second = cache.connector(dir.path(), LoadStrategy::View).unwrap();

    assert_eq!(cache.builds(), 1, "the second connector is a cache hit");
    let fresh = DuckDbConnector::from_directory(dir.path(), LoadStrategy::View).unwrap();
    assert_eq!(tables(&first), tables(&fresh));
    assert_eq!(tables(&second), tables(&fresh));
    assert_eq!(count(&second, "orders").await, 2.0);
    assert_eq!(count(&second, "stores").await, 3.0);
}

#[tokio::test]
async fn a_changed_added_or_removed_file_rebuilds() {
    let dir = sample_dir();
    let cache = DirCache::new(4);
    let before = cache.connector(dir.path(), LoadStrategy::View).unwrap();
    assert_eq!(count(&before, "orders").await, 2.0);

    write_csv(
        dir.path(),
        "orders.csv",
        "order_id,total\n1,10\n2,20\n3,30\n",
    );
    let rewritten = cache.connector(dir.path(), LoadStrategy::View).unwrap();
    assert_eq!(cache.builds(), 2, "a rewritten file rebuilds");
    assert_eq!(count(&rewritten, "orders").await, 3.0);
    // The replaced database lives on for the connector still holding it.
    assert_eq!(count(&before, "stores").await, 3.0);

    // Same bytes, new modification time.
    std::fs::File::options()
        .write(true)
        .open(dir.path().join("orders.csv"))
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000))
        .unwrap();
    cache.connector(dir.path(), LoadStrategy::View).unwrap();
    assert_eq!(cache.builds(), 3, "a touched file rebuilds");

    write_csv(dir.path(), "customers.csv", "customer_id\n7\n");
    let added = cache.connector(dir.path(), LoadStrategy::View).unwrap();
    assert_eq!(cache.builds(), 4, "an added file rebuilds");
    assert_eq!(count(&added, "customers").await, 1.0);

    std::fs::remove_file(dir.path().join("customers.csv")).unwrap();
    let removed = cache.connector(dir.path(), LoadStrategy::View).unwrap();
    assert_eq!(cache.builds(), 5, "a removed file rebuilds");
    assert!(
        removed
            .loaded_tables()
            .iter()
            .all(|t| t.name != "customers")
    );

    cache.connector(dir.path(), LoadStrategy::View).unwrap();
    assert_eq!(cache.builds(), 5, "and unchanged again is a hit");
    assert_eq!(cache.len(), 1, "one entry per directory and strategy");
}

/// Every name a semantic model or a person uses for a file resolves
/// through a clone — including the file-path form, which needs the
/// clone's own `file_search_path`.
#[tokio::test]
async fn a_clone_sees_every_alias() {
    let dir = sample_dir();
    let cache = DirCache::new(4);
    cache.connector(dir.path(), LoadStrategy::View).unwrap();
    let clone = cache.connector(dir.path(), LoadStrategy::View).unwrap();
    assert_eq!(cache.builds(), 1);

    for from in [
        "orders",
        r#""orders.csv""#,
        "orders.csv",
        r#""orders"."csv""#,
        "'orders.csv'",
    ] {
        assert_eq!(count(&clone, from).await, 2.0, "FROM {from}");
    }
    for from in [
        "stores",
        r#""stores.parquet""#,
        "stores.parquet",
        r#""stores"."parquet""#,
        "'stores.parquet'",
    ] {
        assert_eq!(count(&clone, from).await, 3.0, "FROM {from}");
    }
    assert!(
        is_shared(&clone),
        "reads keep the clone on the shared database"
    );
}

#[test]
fn clones_do_not_share_a_connection() {
    let dir = sample_dir();
    let cache = DirCache::new(4);
    let held = cache.connector(dir.path(), LoadStrategy::View).unwrap();
    let other = cache.connector(dir.path(), LoadStrategy::View).unwrap();

    // With `held`'s connection locked, `other` still answers.
    let _guard = held.conn.session().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let res = rt.block_on(other.execute_query("SELECT count(*) FROM orders", 10));
        tx.send(res.map(|r| r.result.rows.len())).unwrap();
    });
    let answered = rx
        .recv_timeout(Duration::from_secs(60))
        .expect("a clone must not wait on another clone's connection");
    assert_eq!(answered.unwrap(), 1);
}

#[test]
fn clones_query_concurrently() {
    let dir = sample_dir();
    let cache = Arc::new(DirCache::new(4));
    let threads = 4;
    let barrier = Arc::new(Barrier::new(threads));
    let workers: Vec<_> = (0..threads)
        .map(|_| {
            let (cache, barrier) = (Arc::clone(&cache), Arc::clone(&barrier));
            let dir = dir.path().to_path_buf();
            thread::spawn(move || {
                let c = cache.connector(&dir, LoadStrategy::View).unwrap();
                let rt = tokio::runtime::Builder::new_current_thread()
                    .build()
                    .unwrap();
                barrier.wait();
                for _ in 0..20 {
                    let res = rt
                        .block_on(c.execute_query(
                            "SELECT o.order_id, s.store_id FROM orders o CROSS JOIN stores s",
                            100,
                        ))
                        .unwrap();
                    assert_eq!(res.result.total_row_count, 6);
                }
            })
        })
        .collect();
    for w in workers {
        w.join().unwrap();
    }
    assert_eq!(cache.builds(), 1);
}

/// What a run writes stays in that run: the writer moves onto a private
/// database first, and neither other clones nor later connectors see it.
#[tokio::test]
async fn a_write_stays_with_the_connector_that_made_it() {
    let dir = sample_dir();
    let cache = DirCache::new(4);
    let writer = cache.connector(dir.path(), LoadStrategy::View).unwrap();
    let reader = cache.connector(dir.path(), LoadStrategy::View).unwrap();

    writer
        .execute_statement("CREATE TABLE scratch AS SELECT 1 AS x")
        .await
        .unwrap();
    // Destructive, on an object every catalog holds: the `"<stem>"."<ext>"`
    // alias lives in a real schema even in a private database. Asked of the
    // catalog: with the view gone, DuckDB's replacement scan still answers
    // `"orders"."csv"` from the file.
    writer
        .execute_query("DROP SCHEMA orders CASCADE", 10)
        .await
        .unwrap();
    assert!(!is_shared(&writer));
    assert_eq!(count(&writer, "scratch").await, 1.0);
    assert_eq!(views_in_orders(&writer).await, 0.0);
    assert_eq!(count(&writer, "stores.parquet").await, 3.0);

    assert!(is_shared(&reader));
    assert!(
        reader
            .execute_query("SELECT * FROM scratch", 10)
            .await
            .is_err()
    );
    assert_eq!(views_in_orders(&reader).await, 1.0);
    assert_eq!(count(&reader, "orders").await, 2.0);

    // A DDL script through `execute_query` (an automation setup file).
    let script = reader
        .execute_query("CREATE TABLE t AS SELECT 5 AS v; SELECT v FROM t", 10)
        .await
        .unwrap();
    assert_eq!(script.result.rows[0].0[0], CellValue::Number(5.0));
    assert!(!is_shared(&reader));

    let later = cache.connector(dir.path(), LoadStrategy::View).unwrap();
    assert!(
        later
            .execute_query("SELECT * FROM scratch", 10)
            .await
            .is_err()
    );
    assert!(later.execute_query("SELECT * FROM t", 10).await.is_err());
    assert_eq!(views_in_orders(&later).await, 1.0);
    assert_eq!(cache.builds(), 1, "a private copy is not a cache build");
}

/// A read the scanner can follow, dollar quotes and backslashes included,
/// stays on the shared clone; a statement it cannot follow is treated as a
/// write and moves the connector to its private copy before it runs.
#[tokio::test]
async fn a_statement_the_scanner_cannot_follow_runs_on_a_private_copy() {
    let dir = sample_dir();
    let cache = DirCache::new(4);
    let reader = cache.connector(dir.path(), LoadStrategy::View).unwrap();
    assert_eq!(
        first_cell(&reader, "SELECT $$it's$$ AS s").await,
        CellValue::Text("it's".into())
    );
    assert_eq!(
        first_cell(&reader, "SELECT 'a\\b' AS s").await,
        CellValue::Text("a\\b".into())
    );
    assert!(is_shared(&reader), "a dollar-quoted read stays shared");

    let unreadable = cache.connector(dir.path(), LoadStrategy::View).unwrap();
    assert_eq!(
        first_cell(&unreadable, "SELECT 1 AS x$y$").await,
        CellValue::Number(1.0)
    );
    assert!(
        !is_shared(&unreadable),
        "a dollar inside a name moved it to a private copy"
    );
    assert_eq!(count(&unreadable, "orders").await, 2.0);
    assert_eq!(cache.builds(), 1);
}

/// The scripts that got a write past the lexer (review F1: a quote inside a
/// dollar quote; F2: a `$` inside a name) leave the catalog another clone
/// reads as it was.
#[tokio::test]
async fn a_write_hidden_from_the_lexer_does_not_reach_another_clone() {
    let dir = sample_dir();
    let cache = DirCache::new(4);
    let reader = cache.connector(dir.path(), LoadStrategy::View).unwrap();
    for script in [
        "WITH a AS (SELECT $$it's$$ AS s) SELECT * FROM a; DROP VIEW stores; SELECT '$$'",
        "SELECT 1 AS x$y$; DROP VIEW stores; SELECT 1 AS x$y$",
    ] {
        let writer = cache.connector(dir.path(), LoadStrategy::View).unwrap();
        writer
            .execute_statement(script)
            .await
            .unwrap_or_else(|e| panic!("{script}: {e}"));
        assert!(!is_shared(&writer), "{script}");
        assert!(
            writer
                .execute_query("SELECT * FROM stores", 10)
                .await
                .is_err(),
            "{script} dropped the writer's own view"
        );
        assert_eq!(count(&reader, "stores").await, 3.0, "{script}");
    }
    assert!(is_shared(&reader));
}

/// With the lexer made to call a write a read, DuckDB's parser still moves
/// the connector to its private copy, and another clone sees nothing.
#[tokio::test]
async fn the_parser_holds_where_the_lexer_is_wrong() {
    let dir = sample_dir();
    let cache = DirCache::new(4);
    let writer = cache.connector(dir.path(), LoadStrategy::View).unwrap();
    let reader = cache.connector(dir.path(), LoadStrategy::View).unwrap();
    for script in [
        "CREATE TABLE scratch AS SELECT 1 AS x",
        "SELECT 1; DROP VIEW stores",
    ] {
        let turn = writer.conn.turn_with(script, |_| true).await.unwrap();
        turn.session()
            .unwrap()
            .execute_batch(script)
            .unwrap_or_else(|e| panic!("{script}: {e}"));
    }
    assert!(!is_shared(&writer));
    // Asked of the catalog: a query naming `scratch` does not bind on the
    // parser's shadow, so it would itself move `reader` to a private copy.
    assert_eq!(
        count(&reader, "duckdb_tables() WHERE table_name = 'scratch'").await,
        0.0
    );
    assert_eq!(count(&reader, "stores").await, 3.0);

    // And a read both agree on stays shared.
    drop(
        reader
            .conn
            .turn_with("SELECT * FROM orders", |_| true)
            .await
            .unwrap(),
    );
    assert!(is_shared(&reader));
}

#[test]
fn a_failed_build_is_not_cached() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("broken.parquet"), b"not a parquet file").unwrap();
    let cache = DirCache::new(4);

    assert!(cache.connector(dir.path(), LoadStrategy::View).is_err());
    assert!(cache.connector(dir.path(), LoadStrategy::View).is_err());
    assert_eq!(cache.builds(), 2, "each attempt builds again");
    assert_eq!(cache.len(), 0);

    write_parquet(dir.path(), "broken.parquet", "SELECT 1 AS id");
    cache.connector(dir.path(), LoadStrategy::View).unwrap();
    assert_eq!(cache.builds(), 3);
    assert_eq!(cache.len(), 1);
    assert_eq!(cache.build_lock_count(), 0);
}

#[test]
fn concurrent_first_requests_for_one_directory_build_once() {
    let dir = sample_dir();
    let cache = Arc::new(DirCache::new(4));
    let threads = 8;
    let barrier = Arc::new(Barrier::new(threads));
    let workers: Vec<_> = (0..threads)
        .map(|_| {
            let (cache, barrier) = (Arc::clone(&cache), Arc::clone(&barrier));
            let dir = dir.path().to_path_buf();
            thread::spawn(move || {
                barrier.wait();
                cache.connector(&dir, LoadStrategy::View).unwrap();
            })
        })
        .collect();
    for w in workers {
        w.join().unwrap();
    }
    assert_eq!(cache.builds(), 1);
    assert_eq!(cache.build_lock_count(), 0, "build locks are released");
}

#[test]
fn the_least_recently_used_directory_is_evicted_at_capacity() {
    let (a, b, c) = (sample_dir(), sample_dir(), sample_dir());
    let cache = DirCache::new(2);
    let get = |d: &tempfile::TempDir| cache.connector(d.path(), LoadStrategy::View).unwrap();

    get(&a);
    get(&b);
    get(&a); // hit: `a` is now the most recently used
    assert_eq!(cache.builds(), 2);
    get(&c); // evicts `b`
    assert_eq!((cache.builds(), cache.len()), (3, 2));
    get(&a);
    assert_eq!(cache.builds(), 3, "`a` survived");
    get(&b);
    assert_eq!(cache.builds(), 4, "`b` was evicted");
    assert_eq!(cache.len(), 2);
}

#[tokio::test]
async fn materialized_tables_follow_their_files() {
    let dir = sample_dir();
    let cache = DirCache::new(4);
    let first = cache
        .connector(dir.path(), LoadStrategy::Materialized)
        .unwrap();
    assert_eq!(count(&first, "orders").await, 2.0);

    write_csv(
        dir.path(),
        "orders.csv",
        "order_id,total\n1,10\n2,20\n3,30\n",
    );
    let second = cache
        .connector(dir.path(), LoadStrategy::Materialized)
        .unwrap();
    assert_eq!(count(&second, "orders").await, 3.0);
    assert_eq!(count(&second, r#""orders"."csv""#).await, 3.0);

    cache.connector(dir.path(), LoadStrategy::View).unwrap();
    assert_eq!(cache.builds(), 3, "each strategy is its own entry");
}

/// Times a real directory: the first connector builds, the second is a
/// hit. `OXY_DUCKDB_CACHE_BENCH_DIR=<dir> cargo nextest run -p
/// agentic-connector --features duckdb --lib --run-ignored only
/// --no-capture measure_`.
#[test]
#[ignore = "measures a real directory named by OXY_DUCKDB_CACHE_BENCH_DIR"]
fn measure_a_directory_build_against_a_cache_hit() {
    let Ok(dir) = std::env::var("OXY_DUCKDB_CACHE_BENCH_DIR") else {
        eprintln!("OXY_DUCKDB_CACHE_BENCH_DIR is not set; nothing to measure");
        return;
    };
    let dir = Path::new(&dir);
    let cache = DirCache::new(4);

    let started = Instant::now();
    let uncached = DuckDbConnector::from_directory(dir, LoadStrategy::View).unwrap();
    let uncached_elapsed = started.elapsed();

    let started = Instant::now();
    let first = cache.connector(dir, LoadStrategy::View).unwrap();
    let first_elapsed = started.elapsed();

    let started = Instant::now();
    let second = cache.connector(dir, LoadStrategy::View).unwrap();
    let second_elapsed = started.elapsed();

    eprintln!(
        "{} tables: from_directory {uncached_elapsed:?}, cached build {first_elapsed:?}, \
         cache hit {second_elapsed:?}",
        second.loaded_tables().len()
    );
    assert_eq!(cache.builds(), 1);
    assert_eq!(tables(&first), tables(&second));
    assert_eq!(tables(&uncached), tables(&second));
}
