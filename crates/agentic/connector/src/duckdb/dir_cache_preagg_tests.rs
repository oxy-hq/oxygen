//! A cached connector under the load a pre-aggregation cycle puts on it:
//! many rollups rebuilding at once through one connector, each writing
//! first. The private copy the first write needs is made slow on purpose, to
//! see what waits on it.

use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use agentic_core::result::CellValue;

use super::*;
use crate::connector::DatabaseConnector;

/// `orders.csv`, 2 rows.
fn sample_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("orders.csv"),
        "order_id,total\n1,10\n2,20\n",
    )
    .unwrap();
    dir
}

/// The statements one rollup rebuild sends: schema and manifest DDL, the
/// rollup table, its manifest row, then the pull.
async fn rebuild_rollup(c: &DuckDbConnector, i: usize, table: &str) {
    for sql in [
        "CREATE SCHEMA IF NOT EXISTS oxy_preagg".to_string(),
        "CREATE TABLE IF NOT EXISTS oxy_preagg.__manifest (h VARCHAR, built TIMESTAMP)".into(),
        format!(
            r#"CREATE OR REPLACE TABLE oxy_preagg.r{i} AS SELECT count(*) AS n FROM "{table}""#
        ),
        format!("INSERT INTO oxy_preagg.__manifest VALUES ('h{i}', now())"),
    ] {
        c.execute_statement(&sql)
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    c.execute_query(&format!("SELECT * FROM oxy_preagg.r{i}"), 500_000)
        .await
        .unwrap();
}

async fn number(c: &DuckDbConnector, sql: &str) -> f64 {
    match c.execute_query(sql, 10).await.unwrap().result.rows[0].0[0] {
        CellValue::Number(n) => n,
        ref other => panic!("{sql} returned {other:?}"),
    }
}

fn is_shared(c: &DuckDbConnector) -> bool {
    c.conn.session().unwrap().is_shared()
}

/// A runtime that counts its own threads — workers and blocking pool, not
/// DuckDB's — and remembers the most alive at once.
fn counted_runtime(workers: usize) -> (tokio::runtime::Runtime, Arc<AtomicUsize>) {
    let live = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .on_thread_start({
            let (live, peak) = (Arc::clone(&live), Arc::clone(&peak));
            move || {
                let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
            }
        })
        .on_thread_stop({
            let live = Arc::clone(&live);
            move || {
                live.fetch_sub(1, Ordering::SeqCst);
            }
        })
        .build()
        .unwrap();
    (rt, peak)
}

/// Forty rollups rebuilding through one cached connector all finish, on one
/// private copy, and the writes queued behind its build wait without a
/// thread each.
#[test]
fn concurrent_rebuilds_on_one_connector_share_one_build_and_hold_no_threads() {
    const ROLLUPS: usize = 40;
    let dir = sample_dir();
    let cache = DirCache::new(4).with_private_build_delay(Duration::from_millis(1500));
    let (rt, peak) = counted_runtime(2);
    rt.block_on(async {
        let c = Arc::new(cache.connector(dir.path(), LoadStrategy::View).unwrap());
        let mut rollups = tokio::task::JoinSet::new();
        for i in 0..ROLLUPS {
            let c = Arc::clone(&c);
            rollups.spawn(async move { rebuild_rollup(&c, i, "orders").await });
        }
        tokio::time::timeout(Duration::from_secs(120), async {
            while let Some(done) = rollups.join_next().await {
                done.unwrap();
            }
        })
        .await
        .expect("every rebuild finishes");

        assert_eq!(
            number(&c, "SELECT count(*) FROM oxy_preagg.__manifest").await,
            ROLLUPS as f64
        );
        assert!(!is_shared(&c));
        let other = cache.connector(dir.path(), LoadStrategy::View).unwrap();
        assert_eq!(
            number(
                &other,
                "SELECT count(*) FROM duckdb_schemas() WHERE schema_name = 'oxy_preagg'"
            )
            .await,
            0.0,
            "another clone sees none of it"
        );
    });
    assert_eq!(cache.private_builds(), 1, "one private copy per connector");
    let peak = peak.load(Ordering::SeqCst);
    assert!(
        peak <= 8,
        "{peak} runtime threads at once for {ROLLUPS} queued rebuilds"
    );
}

/// While a write's private copy builds, a read on the same connector runs on
/// the clone, and the runtime — one worker here — keeps going: the build is
/// on the blocking pool and holds neither the session nor the turn.
#[test]
fn a_private_build_holds_up_neither_reads_on_its_connector_nor_the_runtime() {
    let build = Duration::from_millis(2000);
    let dir = sample_dir();
    let cache = DirCache::new(4).with_private_build_delay(build);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let c = Arc::new(cache.connector(dir.path(), LoadStrategy::View).unwrap());
        let started = Instant::now();
        // An automation setup script: through `execute_query`, which runs on
        // the runtime's own thread.
        let writer = {
            let c = Arc::clone(&c);
            tokio::spawn(async move {
                c.execute_query("CREATE TABLE t AS SELECT 7 AS v; SELECT v FROM t", 10)
                    .await
                    .unwrap()
            })
        };
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(number(&c, "SELECT count(*) FROM orders").await, 2.0);
        let read_at = started.elapsed();
        assert!(
            read_at < build / 2,
            "the read waited {read_at:?} on a {build:?} build"
        );

        let wrote = writer.await.unwrap();
        assert_eq!(wrote.result.rows[0].0[0], CellValue::Number(7.0));
        assert!(started.elapsed() >= build);
        assert!(!is_shared(&c));
        let other = cache.connector(dir.path(), LoadStrategy::View).unwrap();
        assert_eq!(
            number(
                &other,
                "SELECT count(*) FROM duckdb_tables() WHERE table_name = 't'"
            )
            .await,
            0.0
        );
    });
}

/// A pre-aggregation connector (`reuse_cached_directory: false`) is built
/// the way it was before the cache, and never enters it.
#[test]
fn a_connector_that_does_not_reuse_the_cache_never_enters_it() {
    let dir = sample_dir();
    let config = crate::ConnectorConfig::DuckDb(crate::DuckDbConfig {
        data_dir: dir.path().to_path_buf(),
        load_strategy: crate::DuckDbLoadStrategy::View,
        reuse_cached_directory: true,
    });
    let before = global().len();
    let own = crate::build_connector(config.clone().without_cached_directory()).unwrap();
    assert_eq!(global().len(), before, "built outside the cache");
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let res = rt.block_on(own.execute_query("SELECT count(*) FROM orders", 10));
    assert_eq!(res.unwrap().result.rows[0].0[0], CellValue::Number(2.0));

    let _cached = crate::build_connector(config).unwrap();
    assert_eq!(global().len(), before + 1);
}

fn os_threads() -> usize {
    std::process::Command::new("ps")
        .args(["-M", "-p", &std::process::id().to_string()])
        .output()
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .count()
                .saturating_sub(1)
        })
        .unwrap_or(0)
}

/// One cycle: `workspaces` connectors, each rebuilding `rollups` rollups at
/// once. The wall time, and the most OS threads (DuckDB's included) seen.
async fn cycle(
    connect: Arc<dyn Fn() -> DuckDbConnector + Send + Sync>,
    tables: Arc<Vec<String>>,
    workspaces: usize,
    rollups: usize,
) -> (Duration, usize) {
    let peak = Arc::new(AtomicUsize::new(0));
    let done = Arc::new(AtomicUsize::new(0));
    let sampler = {
        let (peak, done) = (Arc::clone(&peak), Arc::clone(&done));
        tokio::spawn(async move {
            while done.load(Ordering::Relaxed) == 0 {
                peak.fetch_max(os_threads(), Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        })
    };
    let started = Instant::now();
    let mut set = tokio::task::JoinSet::new();
    for _ in 0..workspaces {
        let (connect, tables) = (Arc::clone(&connect), Arc::clone(&tables));
        set.spawn(async move {
            // As `get_connector` builds it: once per workspace, off the runtime.
            let c = Arc::new(
                tokio::task::spawn_blocking(move || connect())
                    .await
                    .unwrap(),
            );
            let mut rollup_set = tokio::task::JoinSet::new();
            for i in 0..rollups {
                let (c, table) = (Arc::clone(&c), tables[i % tables.len()].clone());
                rollup_set.spawn(async move { rebuild_rollup(&c, i, &table).await });
            }
            while let Some(r) = rollup_set.join_next().await {
                r.unwrap();
            }
        });
    }
    while let Some(r) = set.join_next().await {
        r.unwrap();
    }
    let elapsed = started.elapsed();
    done.store(1, Ordering::Relaxed);
    sampler.await.unwrap();
    (elapsed, peak.load(Ordering::Relaxed))
}

/// Times a pre-aggregation cycle over a real directory: connectors built
/// uncached (what pre-aggregation uses) against cached clones. The thread
/// count needs macOS `ps -M`. `OXY_DUCKDB_CACHE_BENCH_DIR=<dir> cargo nextest
/// run -p agentic-connector --features duckdb --lib --run-ignored only
/// --no-capture measure_a_preagg`.
#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
#[ignore = "measures a real directory named by OXY_DUCKDB_CACHE_BENCH_DIR"]
async fn measure_a_preagg_cycle_cached_against_uncached() {
    let Ok(dir) = std::env::var("OXY_DUCKDB_CACHE_BENCH_DIR") else {
        eprintln!("OXY_DUCKDB_CACHE_BENCH_DIR is not set; nothing to measure");
        return;
    };
    let env = |name: &str, default: usize| {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let (workspaces, rollups) = (env("OXY_BENCH_WORKSPACES", 9), env("OXY_BENCH_ROLLUPS", 26));
    let dir = Arc::new(Path::new(&dir).to_path_buf());
    let probe = DuckDbConnector::from_directory(&dir, LoadStrategy::View).unwrap();
    let tables = Arc::new(
        probe
            .loaded_tables()
            .iter()
            .map(|t| t.name.clone())
            .collect::<Vec<_>>(),
    );
    drop(probe);
    eprintln!("threads before: {}", os_threads());

    let uncached = {
        let dir = Arc::clone(&dir);
        Arc::new(move || DuckDbConnector::from_directory(&dir, LoadStrategy::View).unwrap())
    };
    let (t, peak) = cycle(uncached, Arc::clone(&tables), workspaces, rollups).await;
    eprintln!("uncached: {workspaces}x{rollups} in {t:?}, peak threads {peak}");

    let cache = Arc::new(DirCache::new(4));
    let cached = {
        let (dir, cache) = (Arc::clone(&dir), Arc::clone(&cache));
        Arc::new(move || cache.connector(&dir, LoadStrategy::View).unwrap())
    };
    let (t, peak) = cycle(cached, Arc::clone(&tables), workspaces, rollups).await;
    eprintln!("cached: {workspaces}x{rollups} in {t:?}, peak threads {peak}");
}

/// DuckDB's parse check binds the query on the shadow, which reads any file
/// the query names, so it runs on the blocking pool — never on the runtime
/// thread, which on a current-thread runtime is the test's own.
#[test]
fn the_parse_check_never_runs_on_a_runtime_thread() {
    let dir = sample_dir();
    let cache = DirCache::new(4);
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let runtime_thread = std::thread::current().id();
    rt.block_on(async {
        let c = cache.connector(dir.path(), LoadStrategy::View).unwrap();
        assert_eq!(number(&c, "SELECT count(*) FROM orders").await, 2.0);
        c.execute_statement("SELECT 1 FROM orders").await.unwrap();
        c.execute_query_full("SELECT * FROM orders").await.unwrap();
        assert!(is_shared(&c), "reads stay on the clone");
    });
    let checked_on = cache.parse_check_threads();
    assert_eq!(checked_on.len(), 3, "one check per read: {checked_on:?}");
    assert!(
        checked_on.iter().all(|t| *t != runtime_thread),
        "a parse check ran on the runtime thread"
    );
}

/// What a read on a clone pays for its turn — the lexer, and the parse check
/// with its hop to the blocking pool. `OXY_DUCKDB_CACHE_BENCH_DIR=<dir> cargo
/// nextest run -p agentic-connector --features duckdb --lib --run-ignored
/// only --no-capture measure_a_read_s_turn`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "measures a real directory named by OXY_DUCKDB_CACHE_BENCH_DIR"]
async fn measure_a_read_s_turn() {
    let Ok(dir) = std::env::var("OXY_DUCKDB_CACHE_BENCH_DIR") else {
        eprintln!("OXY_DUCKDB_CACHE_BENCH_DIR is not set; nothing to measure");
        return;
    };
    let cache = DirCache::new(4);
    let c = cache
        .connector(Path::new(&dir), LoadStrategy::View)
        .unwrap();
    let table = c.loaded_tables()[0].name.clone();
    let sql = format!(r#"SELECT * FROM "{table}" LIMIT 10"#);
    drop(c.conn.turn_for(&sql).await.unwrap());
    let runs = 200;
    let started = Instant::now();
    for _ in 0..runs {
        drop(c.conn.turn_for(&sql).await.unwrap());
    }
    eprintln!("{sql}: {:?} per turn", started.elapsed() / runs);
    assert!(is_shared(&c));
}
