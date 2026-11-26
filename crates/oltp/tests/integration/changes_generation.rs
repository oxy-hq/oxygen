//! A generation bump's tenant side against a real Postgres (OLTP change
//! stream design §5.6): the owner transaction waits out open capturing
//! transactions or changes nothing, and the re-stamp moves old rows in chunks.

use std::time::Duration;

use oxy_oltp::changes::{
    BUMP_ABANDON_SQL, BUMP_PROGRESS_SQL, BUMP_REQUEST_SQL, BUMP_STREAM_SQL, BaselineReason,
    END_RESTAMP_SQL, RESTAMP_CHUNK_SQL, bump_lock_sql, pin_session_sql, request_baseline_sql,
};
use oxy_oltp::platform::{CHANGES_BASELINES_TABLE, CHANGES_LOG_TABLE, CHANGES_STREAM_TABLE};

use crate::changes_baseline::blocked;
use crate::changes_capture::Store;
use crate::render_pg_error;

const STATEMENT_TIMEOUT: Duration = Duration::from_secs(30);

/// A writer's change that leaves its transaction open until `COMMIT`.
const OPEN_WRITE: &str = "BEGIN; INSERT INTO leases VALUES (1, 'a', 'a@x')";

async fn connect(dsn: &str) -> Result<tokio_postgres::Client, String> {
    oxy_oltp::connect::connect(dsn, "changes generation test")
        .await
        .map_err(render_pg_error)
}

/// The bump as the relay runs it, one owner transaction pinned first: the old
/// generation, or `None` when the stream moved no row.
async fn bump(owner_dsn: &str, new: i64, lock_timeout: Duration) -> Result<Option<i64>, String> {
    let mut client = connect(owner_dsn).await?;
    let tx = client.transaction().await.map_err(render_pg_error)?;
    tx.batch_execute(&pin_session_sql(lock_timeout, STATEMENT_TIMEOUT))
        .await
        .map_err(render_pg_error)?;
    for sql in bump_lock_sql(lock_timeout) {
        tx.batch_execute(&sql).await.map_err(render_pg_error)?;
    }
    let Some(row) = tx
        .query_opt(BUMP_STREAM_SQL, &[&new])
        .await
        .map_err(render_pg_error)?
    else {
        tx.rollback().await.map_err(render_pg_error)?;
        return Ok(None);
    };
    let old: i64 = row.get(0);
    tx.execute(BUMP_ABANDON_SQL, &[])
        .await
        .map_err(render_pg_error)?;
    tx.execute(BUMP_PROGRESS_SQL, &[])
        .await
        .map_err(render_pg_error)?;
    tx.execute(BUMP_REQUEST_SQL, &[&new])
        .await
        .map_err(render_pg_error)?;
    tx.commit().await.map_err(render_pg_error)?;
    Ok(Some(old))
}

/// The stream's `generation, restamping`.
async fn stream(store: &Store) -> Result<(i64, Option<i64>), String> {
    let client = connect(&store.fx.owner_dsn).await?;
    let row = client
        .query_one(
            format!("SELECT generation, restamping FROM {CHANGES_STREAM_TABLE}").as_str(),
            &[],
        )
        .await
        .map_err(render_pg_error)?;
    Ok((row.get(0), row.get(1)))
}

/// Every baseline as `state, generation, reason`, in `id` order.
async fn baselines(store: &Store) -> Result<Vec<(String, i64, String)>, String> {
    let client = connect(&store.fx.owner_dsn).await?;
    let sql =
        format!("SELECT state, generation, reason FROM {CHANGES_BASELINES_TABLE} ORDER BY id");
    let rows = client
        .query(sql.as_str(), &[])
        .await
        .map_err(render_pg_error)?;
    Ok(rows
        .iter()
        .map(|r| (r.get(0), r.get(1), r.get(2)))
        .collect())
}

/// The log's `seq, generation` pairs, in `seq` order.
async fn log(store: &Store) -> Result<Vec<(i64, i64)>, String> {
    let client = connect(&store.fx.owner_dsn).await?;
    let sql = format!("SELECT seq, generation FROM {CHANGES_LOG_TABLE} ORDER BY seq");
    let rows = client
        .query(sql.as_str(), &[])
        .await
        .map_err(render_pg_error)?;
    Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
}

/// The `intent_batch, shipped_batch` of every writer, in writer order.
async fn progress(store: &Store) -> Result<Vec<(i64, i64)>, String> {
    let client = connect(&store.fx.owner_dsn).await?;
    let rows = client
        .query(
            "SELECT intent_batch, shipped_batch FROM oxy_changes.progress ORDER BY writer",
            &[],
        )
        .await
        .map_err(render_pg_error)?;
    Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
}

const SEED_PROGRESS: &str =
    "INSERT INTO oxy_changes.progress (writer, intent_batch, shipped_batch) VALUES ('w', 5, 4)";

// ── The lock ─────────────────────────────────────────────────────────────────

async fn waited(
    store: &Store,
) -> Result<(Option<i64>, Vec<(i64, i64)>, (i64, Option<i64>)), String> {
    store.leases(&[]).await?;
    let old = stream(store).await?.0;
    let writer = connect(&store.writer_dsn).await?;
    writer
        .batch_execute(OPEN_WRITE)
        .await
        .map_err(render_pg_error)?;
    let dsn = store.fx.owner_dsn.clone();
    let task = tokio::spawn(async move { bump(&dsn, old + 1, Duration::from_secs(5)).await });
    blocked(&store.fx.owner_dsn, &bump_lock_sql(Duration::ZERO)[1]).await?;
    writer
        .batch_execute("COMMIT")
        .await
        .map_err(render_pg_error)?;
    let bumped = task.await.map_err(|e| e.to_string())??;
    let before = log(store).await?;
    store
        .as_writer("INSERT INTO leases VALUES (2, 'b', 'b@x')")
        .await?;
    let after = log(store).await?;
    let new_rows = after.into_iter().skip(before.len()).collect::<Vec<_>>();
    Ok((
        bumped.map(|o| o - old),
        [before, new_rows].concat(),
        stream(store).await?,
    ))
}

#[tokio::test]
async fn a_bump_waits_out_an_open_capturing_transaction() {
    let Some(store) = Store::create("gen_wait").await else {
        return;
    };
    let out = waited(&store).await;
    store.cleanup().await;

    let (bumped, rows, state) = out.expect("write, bump, commit, write");
    assert_eq!(bumped, Some(0), "the bump returns the old generation");
    let new = state.0;
    assert_eq!(state.1, Some(new - 1), "re-stamping the old generation");
    let generations: Vec<i64> = rows.iter().map(|r| r.1).collect();
    assert_eq!(
        generations.first(),
        Some(&(new - 1)),
        "the waited-for row keeps the old generation: {rows:?}"
    );
    assert_eq!(
        generations.last(),
        Some(&new),
        "a later write is new: {rows:?}"
    );
}

/// Everything a refused bump must leave alone, as text.
async fn snapshot(store: &Store) -> Result<String, String> {
    Ok(format!(
        "{:?} {:?} {:?}",
        stream(store).await?,
        baselines(store).await?,
        progress(store).await?
    ))
}

async fn timed_out(store: &Store) -> Result<(String, [String; 2]), String> {
    store.leases(&[]).await?;
    store.as_owner(SEED_PROGRESS).await?;
    let capture = store.shape("leases", &[]).await?;
    store
        .as_owner(&request_baseline_sql(capture.shape.relid, BaselineReason::OptIn).join("; "))
        .await?;
    let before = snapshot(store).await?;
    let writer = connect(&store.writer_dsn).await?;
    writer
        .batch_execute(OPEN_WRITE)
        .await
        .map_err(render_pg_error)?;
    let old = stream(store).await?.0;
    let err = bump(&store.fx.owner_dsn, old + 1, Duration::from_millis(200)).await;
    writer
        .batch_execute("COMMIT")
        .await
        .map_err(render_pg_error)?;
    let after = snapshot(store).await?;
    Ok((err.expect_err("the bump times out"), [before, after]))
}

#[tokio::test]
async fn a_bump_behind_a_long_transaction_times_out_and_changes_nothing() {
    let Some(store) = Store::create("gen_timeout").await else {
        return;
    };
    let out = timed_out(&store).await;
    store.cleanup().await;

    let (err, [before, after]) = out.expect("write, bump, commit");
    assert!(err.contains("55P03"), "{err}");
    assert_eq!(before, after, "nothing changed");
}

// ── What a bump does ─────────────────────────────────────────────────────────

async fn requested(store: &Store) -> Result<Vec<(String, i64, String)>, String> {
    let leases = store.leases(&[]).await?;
    let tenants = {
        store
            .as_writer("CREATE TABLE tenants (id bigint PRIMARY KEY, name text)")
            .await?;
        let capture = store.shape("tenants", &[]).await?;
        store.install(&capture, 1).await?;
        capture
    };
    for capture in [&leases, &tenants] {
        let sql = request_baseline_sql(capture.shape.relid, BaselineReason::OptIn).join("; ");
        store.as_owner(&sql).await?;
    }
    store
        .as_owner(&format!(
            "UPDATE {CHANGES_BASELINES_TABLE} SET state = 'copying' \
             WHERE relid = {}::oid",
            tenants.shape.relid
        ))
        .await?;
    let old = stream(store).await?.0;
    bump(&store.fx.owner_dsn, old + 1, Duration::from_secs(5)).await?;
    baselines(store).await
}

#[tokio::test]
async fn a_bump_abandons_open_baselines_and_requests_every_installed_table() {
    let Some(store) = Store::create("gen_requests").await else {
        return;
    };
    let out = requested(&store).await;
    store.cleanup().await;

    let rows = out.expect("install two tables, request, bump");
    let old = rows[0].1;
    let expected = |state: &str, generation: i64, reason: &str| {
        (state.to_string(), generation, reason.to_string())
    };
    assert_eq!(
        rows,
        [
            expected("abandoned", old, "opt_in"),
            expected("abandoned", old, "opt_in"),
            expected("requested", old + 1, "bump"),
            expected("requested", old + 1, "bump"),
        ]
    );
}

async fn reset(store: &Store) -> Result<[Vec<(i64, i64)>; 2], String> {
    store.leases(&[]).await?;
    store.as_owner(SEED_PROGRESS).await?;
    let before = progress(store).await?;
    let old = stream(store).await?.0;
    bump(&store.fx.owner_dsn, old + 1, Duration::from_secs(5)).await?;
    Ok([before, progress(store).await?])
}

#[tokio::test]
async fn a_bump_resets_every_writers_progress() {
    let Some(store) = Store::create("gen_progress").await else {
        return;
    };
    let out = reset(&store).await;
    store.cleanup().await;

    let [before, after] = out.expect("seed progress, bump");
    assert_eq!(before, [(5, 4)]);
    assert_eq!(after, [(0, 0)]);
}

async fn second(store: &Store) -> Result<[Option<i64>; 2], String> {
    store.leases(&[]).await?;
    let old = stream(store).await?.0;
    let first = bump(&store.fx.owner_dsn, old + 1, Duration::from_secs(5)).await?;
    let again = bump(&store.fx.owner_dsn, old + 2, Duration::from_secs(5)).await?;
    Ok([first, again])
}

#[tokio::test]
async fn a_second_bump_waits_for_the_restamp() {
    let Some(store) = Store::create("gen_second").await else {
        return;
    };
    let out = second(&store).await;
    store.cleanup().await;

    let [first, again] = out.expect("bump twice");
    assert!(first.is_some(), "the first bump moves the stream");
    assert_eq!(again, None, "no row while restamping");
}

// ── The re-stamp ─────────────────────────────────────────────────────────────

async fn restamped(
    store: &Store,
) -> Result<(Vec<u64>, Vec<(i64, i64)>, Vec<(i64, i64)>, i64, Option<i64>), String> {
    store.leases(&[]).await?;
    for id in 1..=5 {
        store
            .as_writer(&format!("INSERT INTO leases VALUES ({id}, 'a', 'a@x')"))
            .await?;
    }
    let before = log(store).await?;
    let old = stream(store).await?.0;
    bump(&store.fx.owner_dsn, old + 1, Duration::from_secs(5)).await?;
    let client = connect(&store.fx.owner_dsn).await?;
    let mut moved = Vec::new();
    for _ in 0..4 {
        let n = client
            .execute(RESTAMP_CHUNK_SQL, &[&old, &(old + 1), &2_i64])
            .await
            .map_err(render_pg_error)?;
        moved.push(n);
    }
    let after = log(store).await?;
    let ended = client
        .execute(END_RESTAMP_SQL, &[&old])
        .await
        .map_err(render_pg_error)?;
    Ok((
        moved,
        before,
        after,
        old,
        stream(store).await?.1.filter(|_| ended == 1),
    ))
}

#[tokio::test]
async fn restamping_moves_old_rows_in_chunks_and_ends() {
    let Some(store) = Store::create("gen_restamp").await else {
        return;
    };
    let out = restamped(&store).await;
    store.cleanup().await;

    let (moved, before, after, old, restamping) = out.expect("write, bump, restamp, end");
    assert_eq!(moved, [2, 2, 1, 0]);
    let seqs = |rows: &[(i64, i64)]| rows.iter().map(|r| r.0).collect::<Vec<_>>();
    assert_eq!(seqs(&before), seqs(&after), "each row keeps its seq");
    assert!(before.iter().all(|r| r.1 == old), "{before:?}");
    assert!(after.iter().all(|r| r.1 == old + 1), "{after:?}");
    assert_eq!(restamping, None, "the end clears restamping");
}
