//! `sync`: the app writer's `policy_overrides` → Parquet through DuckDB, and
//! the claim that the copy never locks the app's own writes out.
//!
//! Copying a few rows takes microseconds, so "write while it copies" would pass
//! whether or not anything locks. Instead the DuckDB read transaction is held
//! open on purpose and the writes go through while it is: the copy's snapshot
//! is pinned, and the lock it holds is shown to be real (it does block an
//! `ACCESS EXCLUSIVE` request) yet compatible with every row write.

use std::path::Path;
use std::time::Duration;

use oxy_policy_engine::OltpWriter;
use oxy_policy_engine::sync::{self, PgSource};
use tokio_postgres::Client;
use tokio_postgres::error::SqlState;

use crate::common::{self, SCHEMA, SPRING_SALE};

/// The rows of a Parquet file, as `(policy_id, policy id inside the JSON)`.
fn read_back(path: &Path) -> Vec<(String, String)> {
    let conn = duckdb::Connection::open_in_memory().expect("duckdb");
    let mut stmt = conn
        .prepare(&format!(
            "SELECT policy_id, json_extract_string(policy, '$.id') FROM read_parquet('{}') ORDER BY 1",
            path.display()
        ))
        .expect("prepare");
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows")
}

fn temp_files_in(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .expect("read dir")
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".tmp"))
        .collect()
}

/// Run `sql` as the writer, in its own transaction with a short
/// `lock_timeout`, so a write that would wait on a lock fails fast instead of
/// passing slowly.
async fn write_with_lock_timeout(
    writer: &OltpWriter,
    sql: &str,
) -> Result<(), tokio_postgres::Error> {
    let mut client = common::connect(writer).await;
    let tx = client.transaction().await?;
    tx.batch_execute("SET LOCAL lock_timeout = '500ms'").await?;
    tx.batch_execute(sql).await?;
    tx.commit().await
}

/// The whole sync as `writer`, off the async runtime.
async fn sync_as(writer: &OltpWriter, out: &Path) -> Result<u64, sync::SyncError> {
    let (w, target) = (writer.clone(), out.to_path_buf());
    tokio::task::spawn_blocking(move || sync::sync_overrides(&w, &target))
        .await
        .expect("join")
}

#[tokio::test]
async fn sync_writes_every_row_to_parquet_and_leaves_no_temp_file() {
    let t = common::start().await;
    let other = SPRING_SALE.replace("spring_sale", "autumn");
    common::upsert(&t.app, "autumn", &other).await;
    let dir = tempdir();
    let out = dir.join("local.parquet");

    let rows = sync_as(&t.writer, &out).await.expect("syncs");

    assert_eq!(rows, 2);
    assert_eq!(
        read_back(&out),
        vec![
            ("autumn".into(), "autumn".into()),
            ("spring_sale".into(), "spring_sale".into())
        ]
    );
    assert!(temp_files_in(&dir).is_empty(), "{:?}", temp_files_in(&dir));
}

#[tokio::test]
async fn an_open_sync_snapshot_does_not_block_writers() {
    let t = common::start().await;
    let dir = tempdir();
    let out = dir.join("local.parquet");
    let source = PgSource::from_writer(&t.writer).expect("writer dsn");
    let table = sync::overrides_table(&source);
    let conn = sync::open_attached(&source).expect("attach");

    // Open the DuckDB transaction and read through it: from here until COMMIT
    // the Postgres side holds one transaction and its snapshot.
    conn.execute_batch("BEGIN").expect("begin");
    let seen: i64 = conn
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .expect("read");
    assert_eq!(seen, 1);

    let reader = duckdb_backend(&t.admin).await;
    assert_eq!(
        locks_held_by(&t.admin, reader).await,
        vec!["AccessShareLock".to_string()],
        "the sync's only lock on policy_overrides"
    );

    // The app's writes go through while that transaction is open...
    let other = SPRING_SALE.replace("spring_sale", "autumn");
    write_with_lock_timeout(
        &t.writer,
        &format!(
            "INSERT INTO policy_overrides (policy_id, policy) VALUES ('autumn', '{other}'::jsonb)"
        ),
    )
    .await
    .expect("an INSERT does not wait on the sync");
    write_with_lock_timeout(
        &t.writer,
        "UPDATE policy_overrides SET updated_at = now() WHERE policy_id = 'spring_sale'",
    )
    .await
    .expect("an UPDATE does not wait on the sync");

    // ...and the lock is real: an exclusive request does wait on it, so the
    // two successes above are not just the absence of any lock at all.
    let exclusive = write_with_lock_timeout(
        &t.writer,
        "LOCK TABLE policy_overrides IN ACCESS EXCLUSIVE MODE",
    )
    .await
    .expect_err("ACCESS EXCLUSIVE conflicts with the sync's read");
    assert_eq!(
        exclusive.code(),
        Some(&SqlState::LOCK_NOT_AVAILABLE),
        "{exclusive:?}"
    );

    // The copy, inside the same transaction, writes the snapshot it began with.
    let rows = sync::copy_overrides(&conn, &table, &out).expect("copies");
    conn.execute_batch("COMMIT").expect("commit");
    assert_eq!(rows, 1, "the snapshot predates the INSERT");
    assert_eq!(
        read_back(&out),
        vec![("spring_sale".into(), "spring_sale".into())]
    );

    // Once it commits, a fresh sync sees both rows.
    let rows = sync::copy_overrides(&conn, &table, &out).expect("copies again");
    assert_eq!(rows, 2);
}

#[tokio::test]
async fn sync_completes_while_a_writer_holds_uncommitted_changes() {
    let t = common::start().await;
    let mut app = common::connect(&t.writer).await;
    let writer = app.transaction().await.expect("begin");
    writer
        .batch_execute(
            "UPDATE policy_overrides SET policy = jsonb_set(policy, '{bounds,maxMultiplier}', '9')",
        )
        .await
        .expect("uncommitted update");

    let dir = tempdir();
    let out = dir.join("local.parquet");
    let rows = tokio::time::timeout(Duration::from_secs(60), sync_as(&t.writer, &out))
        .await
        .expect("the sync does not wait on the writer")
        .expect("syncs");
    assert_eq!(rows, 1);

    let conn = duckdb::Connection::open_in_memory().expect("duckdb");
    let max: f64 = conn
        .query_row(
            &format!(
                "SELECT CAST(json_extract(policy, '$.bounds.maxMultiplier') AS DOUBLE) FROM read_parquet('{}')",
                out.display()
            ),
            [],
            |r| r.get(0),
        )
        .expect("read back");
    assert_eq!(
        max, 1.1,
        "the committed value, not the writer's uncommitted 9"
    );
    writer.rollback().await.expect("rollback");
}

#[tokio::test]
async fn a_failed_sync_keeps_the_previous_file() {
    let t = common::start().await;
    let dir = tempdir();
    let out = dir.join("local.parquet");
    sync_as(&t.writer, &out).await.expect("first sync");
    let before = std::fs::read(&out).expect("file");

    t.app
        .batch_execute("DROP TABLE policy_overrides")
        .await
        .expect("the writer drops its own table");
    sync_as(&t.writer, &out).await.expect_err("nothing to copy");

    assert_eq!(std::fs::read(&out).expect("file"), before, "untouched");
    assert!(temp_files_in(&dir).is_empty(), "{:?}", temp_files_in(&dir));
}

/// The one other backend with a transaction open: DuckDB's connection.
async fn duckdb_backend(admin: &Client) -> i32 {
    let pids: Vec<i32> = admin
        .query(
            "SELECT pid FROM pg_stat_activity
             WHERE datname = current_database() AND pid <> pg_backend_pid()
               AND state = 'idle in transaction'",
            &[],
        )
        .await
        .expect("pg_stat_activity")
        .iter()
        .map(|r| r.get(0))
        .collect();
    assert_eq!(pids.len(), 1, "exactly one open read transaction: {pids:?}");
    pids[0]
}

async fn locks_held_by(admin: &Client, pid: i32) -> Vec<String> {
    admin
        .query(
            &format!(
                "SELECT DISTINCT mode FROM pg_locks
                 WHERE pid = $1 AND granted AND relation = '{SCHEMA}.policy_overrides'::regclass"
            ),
            &[&pid],
        )
        .await
        .expect("pg_locks")
        .iter()
        .map(|r| r.get(0))
        .collect()
}

fn tempdir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "oxy-policy-sync-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

/// A writer DSN asking for TLS (Neon's `sslmode=require`) is attached as
/// `verify-full`, so a server that cannot prove itself — this container offers
/// no TLS at all — is refused rather than sent the password in the clear.
///
/// DuckDB replaces libpq's reason with "Unknown exception in
/// ExecutorTask::Execute", so the test pins the refusal to the attach step and
/// controls for everything else: the identical DSN with `sslmode=disable`
/// syncs. (libpq on the same conninfo says "server does not support SSL, but
/// SSL was required".)
#[tokio::test]
async fn a_tls_dsn_is_refused_by_a_server_that_cannot_prove_itself() {
    let t = common::start().await;
    assert!(t.writer.dsn().contains("sslmode=disable"));
    let tls = t.writer.dsn().replace("sslmode=disable", "sslmode=require");
    let writer = OltpWriter::from_dsn(&tls).expect("still a writer DSN");
    let dir = tempdir();
    let out = dir.join("local.parquet");

    let err = sync_as(&writer, &out)
        .await
        .expect_err("no verified TLS, no sync");
    assert!(
        matches!(
            err,
            sync::SyncError::NoCaBundle | sync::SyncError::DuckDb { step: "attach", .. }
        ),
        "{err}"
    );
    assert!(!err.to_string().contains(&t.writer_password), "{err}");
    assert!(!out.exists());

    sync_as(&t.writer, &out)
        .await
        .expect("the same DSN without TLS syncs");
}
