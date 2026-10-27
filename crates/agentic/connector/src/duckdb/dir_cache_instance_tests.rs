//! What a clone can do to the database every clone of its entry shares,
//! without writing a row: DuckDB's instance-level settings and its worker
//! pool.

use agentic_core::result::CellValue;

use super::tests::{first_cell, is_shared, sample_dir};
use super::*;
use crate::connector::DatabaseConnector;

async fn number(c: &DuckDbConnector, sql: &str) -> f64 {
    match first_cell(c, sql).await {
        CellValue::Number(n) => n,
        other => panic!("{sql} returned {other:?}"),
    }
}

/// `enable_logging()` is a `SELECT` to DuckDB's parser, and it turns logging
/// on for the whole database: run on the shared one, every later run would
/// log into a store that only grows. It moves its connector to a private
/// copy instead, however the call is reached.
#[tokio::test]
async fn an_instance_function_does_not_reach_another_clone() {
    let dir = sample_dir();
    let cache = DirCache::new(4);
    let probe = "SELECT current_setting('enable_logging')::INT";
    for script in [
        "SELECT * FROM enable_logging()",
        "SELECT * FROM query('SELECT * FROM enable_' || 'logging()')",
    ] {
        let writer = cache.connector(dir.path(), LoadStrategy::View).unwrap();
        writer
            .execute_query(script, 10)
            .await
            .unwrap_or_else(|e| panic!("{script}: {e}"));
        assert!(!is_shared(&writer), "{script} ran on the shared database");
        assert_eq!(number(&writer, probe).await, 1.0, "{script} ran nowhere");
        let reader = cache.connector(dir.path(), LoadStrategy::View).unwrap();
        assert_eq!(number(&reader, probe).await, 0.0, "{script}");
        assert!(is_shared(&reader));
    }
    assert_eq!(cache.builds(), 1);
}

/// A shared database lives as long as the process, so its worker pool does
/// too: it is bounded, where DuckDB's default is one thread per core.
#[tokio::test]
async fn a_shared_database_runs_on_a_bounded_pool() {
    let dir = sample_dir();
    let cache = DirCache::new(4);
    let clone = cache.connector(dir.path(), LoadStrategy::View).unwrap();
    let threads = number(&clone, "SELECT current_setting('threads')::INT").await;
    assert!(
        (1.0..=SHARED_THREADS as f64).contains(&threads),
        "{threads} threads"
    );
    assert!(is_shared(&clone));
}
