//! The tenant side of a relay batch against a real Postgres: what a batch
//! reads, how its number is declared and confirmed, and that deleting what
//! shipped never touches a row that committed after the read.

use oxy_oltp::changes::{
    CONFIRM_SQL, DECLARE_INTENT_SQL, DELETE_SHIPPED_SQL, LogRow, READ_BATCH_SQL, STREAM_SQL,
    StreamState,
};

use crate::changes_capture::Store;
use crate::render_pg_error;

async fn connect(dsn: &str) -> Result<tokio_postgres::Client, String> {
    oxy_oltp::connect::connect(dsn, "changes relay test")
        .await
        .map_err(render_pg_error)
}

/// The stream's generation, as the relay reads it.
async fn generation(store: &Store) -> Result<i64, String> {
    let client = connect(&store.fx.owner_dsn).await?;
    let row = client
        .query_one(STREAM_SQL, &[])
        .await
        .map_err(render_pg_error)?;
    Ok(StreamState::from_row(&row)
        .map_err(render_pg_error)?
        .generation)
}

/// One batch of the store's writer, as the relay reads it.
async fn read_batch(store: &Store, held: &[&str], limit: i64) -> Result<Vec<LogRow>, String> {
    let generation = generation(store).await?;
    let held: Vec<String> = held.iter().map(|t| t.to_string()).collect();
    let client = connect(&store.fx.owner_dsn).await?;
    let rows = client
        .query(
            READ_BATCH_SQL,
            &[&generation, &store.writer, &held, &limit, &i64::MAX],
        )
        .await
        .map_err(render_pg_error)?;
    rows.iter().map(LogRow::from_row).collect()
}

async fn declare(store: &Store, writer: &str) -> Result<i64, String> {
    let client = connect(&store.fx.owner_dsn).await?;
    let row = client
        .query_one(DECLARE_INTENT_SQL, &[&writer])
        .await
        .map_err(render_pg_error)?;
    Ok(row.get::<_, i64>(0))
}

async fn confirm(store: &Store, writer: &str, batch: i64) -> Result<u64, String> {
    let client = connect(&store.fx.owner_dsn).await?;
    client
        .execute(CONFIRM_SQL, &[&writer, &batch])
        .await
        .map_err(render_pg_error)
}

fn ids(rows: &[LogRow]) -> Vec<i64> {
    rows.iter()
        .map(|r| {
            r.row_image
                .as_deref()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok())
                .and_then(|v| v["id"].as_i64())
                .expect("an image with an id")
        })
        .collect()
}

// ── A batch ends at a transaction's end ──────────────────────────────────────

async fn two_transactions(store: &Store) -> Result<Vec<LogRow>, String> {
    store.leases(&[]).await?;
    store
        .as_writer("INSERT INTO leases VALUES (1, 'a', 'a@x'), (2, 'b', 'b@x'), (3, 'c', 'c@x')")
        .await?;
    store
        .as_writer("INSERT INTO leases VALUES (4, 'd', 'd@x'), (5, 'e', 'e@x')")
        .await?;
    read_batch(store, &[], 2).await
}

#[tokio::test]
async fn a_batch_ends_at_its_last_transactions_end() {
    let Some(store) = Store::create("relay_txn_end").await else {
        return;
    };
    let out = two_transactions(&store).await;
    store.cleanup().await;

    let rows = out.expect("read a batch");
    assert_eq!(ids(&rows), [1, 2, 3], "limit 2 ships all of A, none of B");
    assert!(rows.iter().all(|r| r.txid == rows[0].txid), "{rows:#?}");
    assert!(rows.iter().all(|r| r.op == 'i'), "{rows:#?}");
}

// ── Only committed rows are read ─────────────────────────────────────────────

async fn open_transaction(store: &Store) -> Result<Vec<LogRow>, String> {
    store.leases(&[]).await?;
    let writer = connect(&store.writer_dsn).await?;
    writer
        .batch_execute("BEGIN; INSERT INTO leases VALUES (1, 'a', 'a@x')")
        .await
        .map_err(render_pg_error)?;
    let rows = read_batch(store, &[], 10).await?;
    writer
        .batch_execute("ROLLBACK")
        .await
        .map_err(render_pg_error)?;
    Ok(rows)
}

#[tokio::test]
async fn an_uncommitted_transaction_is_not_read() {
    let Some(store) = Store::create("relay_uncommitted").await else {
        return;
    };
    let out = open_transaction(&store).await;
    store.cleanup().await;

    assert_eq!(out.expect("read a batch"), []);
}

// ── Held tables ──────────────────────────────────────────────────────────────

async fn two_tables(store: &Store) -> Result<Vec<LogRow>, String> {
    store.leases(&[]).await?;
    store
        .as_writer("CREATE TABLE tenants (id bigint PRIMARY KEY, name text)")
        .await?;
    let capture = store.shape("tenants", &[]).await?;
    store.install(&capture, 1).await?;
    store
        .as_writer("INSERT INTO leases VALUES (1, 'a', 'a@x')")
        .await?;
    store
        .as_writer("INSERT INTO tenants VALUES (2, 'b')")
        .await?;
    read_batch(store, &["leases"], 10).await
}

#[tokio::test]
async fn held_tables_are_not_read() {
    let Some(store) = Store::create("relay_held").await else {
        return;
    };
    let out = two_tables(&store).await;
    store.cleanup().await;

    let rows = out.expect("read a batch");
    assert_eq!(ids(&rows), [2], "{rows:#?}");
    assert_eq!(rows[0].table_name, "tenants");
}

// ── Intent and confirm ───────────────────────────────────────────────────────

async fn three_intents(store: &Store) -> Result<[i64; 3], String> {
    let first = declare(store, "w1").await?;
    let second = declare(store, "w2").await?;
    let third = declare(store, "w1").await?;
    Ok([first, second, third])
}

#[tokio::test]
async fn intent_numbers_only_grow_across_writers() {
    let Some(store) = Store::create("relay_intent").await else {
        return;
    };
    let out = three_intents(&store).await;
    store.cleanup().await;

    let [first, second, third] = out.expect("declare intents");
    assert!(first < second && second < third, "{first} {second} {third}");
}

async fn stale_then_current(store: &Store) -> Result<(u64, u64), String> {
    let b1 = declare(store, "w1").await?;
    let b2 = declare(store, "w1").await?;
    Ok((
        confirm(store, "w1", b1).await?,
        confirm(store, "w1", b2).await?,
    ))
}

#[tokio::test]
async fn confirm_needs_the_intent_it_declared() {
    let Some(store) = Store::create("relay_confirm").await else {
        return;
    };
    let out = stale_then_current(&store).await;
    store.cleanup().await;

    assert_eq!(out.expect("declare and confirm"), (0, 1));
}

// ── Delete exactly what shipped ──────────────────────────────────────────────

/// A transaction holding the lower `seq` stays open while a later one is read
/// and shipped; it commits before the delete.
async fn late_commit(store: &Store) -> Result<(Vec<i64>, Vec<i64>), String> {
    store.leases(&[]).await?;
    let open = connect(&store.writer_dsn).await?;
    open.batch_execute("BEGIN; INSERT INTO leases VALUES (1, 'late', 'l@x')")
        .await
        .map_err(render_pg_error)?;
    store
        .as_writer("INSERT INTO leases VALUES (2, 'early', 'e@x')")
        .await?;

    let batch = read_batch(store, &[], 10).await?;
    open.batch_execute("COMMIT")
        .await
        .map_err(render_pg_error)?;

    let seqs: Vec<i64> = batch.iter().map(|r| r.seq).collect();
    let generation = generation(store).await?;
    let owner = connect(&store.fx.owner_dsn).await?;
    owner
        .execute(DELETE_SHIPPED_SQL, &[&generation, &seqs])
        .await
        .map_err(render_pg_error)?;
    let left = read_batch(store, &[], 10).await?;
    Ok((ids(&batch), ids(&left)))
}

#[tokio::test]
async fn a_row_committed_after_the_read_is_not_deleted() {
    let Some(store) = Store::create("relay_late_commit").await else {
        return;
    };
    let out = late_commit(&store).await;
    store.cleanup().await;

    let (shipped, left) = out.expect("read, commit, delete");
    assert_eq!(shipped, [2], "the open transaction is not in the batch");
    assert_eq!(left, [1], "its row, committed after the read, survives");
}
