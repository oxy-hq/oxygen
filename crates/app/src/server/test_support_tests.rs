//! Repro for the deadlock-shaped bug `AdvisoryLock::acquire` (in
//! `test_support.rs`) exists to prevent, and proof the fix holds.
//!
//! # The bug
//!
//! A *blocking* `SELECT pg_advisory_lock(key)` is, for as long as it waits,
//! an open implicit transaction — a snapshot Postgres has not yet released.
//! `CREATE INDEX CONCURRENTLY` (several of the migrations this lock
//! serializes run one, e.g. `m20260911_000002_function_failure_fingerprint_index`)
//! must wait for every *other* session's open snapshot to clear before it
//! can finish, regardless of whether that session ever touches the table
//! being indexed. So a session merely *waiting* to take this lock can stall
//! an unrelated `CREATE INDEX CONCURRENTLY` elsewhere in the same database —
//! and when the lock holder is itself the one running that statement (the
//! real migration-bootstrap shape in `test_db()`/`cli::commands::serve`),
//! the wait becomes circular: the holder can't finish without the waiter's
//! snapshot clearing, and the waiter's snapshot can't clear until the
//! holder releases. That snapshot wait IS a real, trackable lock-manager
//! wait — `WaitForOlderSnapshots` goes through `VirtualXactLock` and shows
//! up in `pg_locks` as a `virtualxid` wait — so Postgres's deadlock detector
//! isn't blind to it. What it can't see is the OTHER leg: the advisory lock
//! sits on its own idle, separate connection, which isn't waiting on
//! anything from Postgres's point of view. "This connection only releases
//! once the OTHER connection's migration finishes" is a dependency the
//! application enforces, not one any backend-to-backend wait expresses —
//! so the cycle has no edge back to the holder, and the detector reports
//! nothing wrong. Observed 2026-10-02: six `kind(lib)` tests hung ~2,400s
//! this way before panicking.
//!
//! # What this test shows
//!
//! Against a scratch database of its own (never the shared `OXY_DATABASE_URL`
//! target, so this can't interleave with any other DB-backed test's locks,
//! tables, or the shared migration bootstrap):
//!
//! 1. Connection A takes and holds an advisory lock.
//! 2. Connection B tries to take the SAME lock while A holds it — either
//!    the OLD way (a raw blocking `pg_advisory_lock`, standing in for the
//!    code this module no longer has — see `test_support.rs` history) or
//!    the NEW way ([`AdvisoryLock::acquire`], the actual code under test).
//! 3. Connection C runs `CREATE INDEX CONCURRENTLY` on an unrelated table,
//!    under a short `statement_timeout` so a failure here costs seconds,
//!    not the 2,400s the real hang took to panic.
//!
//! With the OLD blocking B, C times out — reproducing the cycle. With the
//! NEW polling B, C completes — proving a waiter no longer stalls a
//! concurrent index build elsewhere.
//!
//! Skips (doesn't fail) when `OXY_DATABASE_URL` is unset, same as every
//! other DB-backed test in this crate.

use sqlx::Connection;
use sqlx::postgres::PgConnection;
use std::time::Duration;

use super::{AdvisoryLock, SKIP_MSG, database_url};

/// Gives a just-started waiter time to actually reach its first blocking (or
/// first polling) statement before connection C starts, so the comparison
/// measures steady-state behaviour rather than a connection-setup race.
const WAITER_SETTLE: Duration = Duration::from_millis(500);

/// Short enough that connection C's `CREATE INDEX CONCURRENTLY` fails fast
/// when stuck (seconds, not the 2,400s the real hang took), long enough that
/// a healthy build on an empty table never comes close to it.
const SET_STATEMENT_TIMEOUT: &str = "SET statement_timeout = '3s'";

fn scratch_db_name() -> String {
    // Process id plus a UUID: readable enough to spot and drop by hand, and
    // collision-proof even against a previous run's crash leftovers on a
    // persistent (non-CI) local Postgres.
    format!(
        "advisory_lock_poll_{}_{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    )
}

/// `admin_url` with its database swapped for `db_name`, preserving
/// authority/query string.
fn with_db(admin_url: &str, db_name: &str) -> String {
    let mut parsed =
        url::Url::parse(admin_url).unwrap_or_else(|e| panic!("OXY_DATABASE_URL is a URL: {e}"));
    parsed.set_path(db_name);
    parsed.into()
}

async fn connect(url: &str) -> PgConnection {
    PgConnection::connect(url)
        .await
        .unwrap_or_else(|e| panic!("connect to {url}: {e}"))
}

/// Runs a fixed (`'static`) SQL statement with no parameters.
async fn exec(conn: &mut PgConnection, sql: &'static str) {
    sqlx::query(sql)
        .execute(conn)
        .await
        .unwrap_or_else(|e| panic!("exec {sql:?}: {e}"));
}

/// Runs a statement built at runtime (a database name interpolated into DDL,
/// which Postgres gives no way to bind as a parameter). `sql` is built from
/// this test's own generated scratch-database name, never external input, so
/// the injection audit `sqlx::AssertSqlSafe` demands is: this file is the
/// only writer of that string.
async fn exec_dynamic(conn: &mut PgConnection, sql: String) {
    sqlx::query(sqlx::AssertSqlSafe(sql.clone()))
        .execute(conn)
        .await
        .unwrap_or_else(|e| panic!("exec {sql:?}: {e}"));
}

#[tokio::test]
async fn waiter_poll_does_not_stall_concurrent_index_build() {
    let Some(admin_url) = database_url() else {
        eprintln!("{SKIP_MSG}");
        return;
    };

    let scratch = scratch_db_name();
    let scratch_url = with_db(&admin_url, &scratch);

    let mut admin = connect(&admin_url).await;
    exec_dynamic(&mut admin, format!("CREATE DATABASE \"{scratch}\"")).await;

    let mut setup = connect(&scratch_url).await;
    exec(&mut setup, "CREATE TABLE t_old (id int)").await;
    exec(&mut setup, "CREATE TABLE t_new (id int)").await;
    let _ = setup.close().await;

    old_blocking_waiter_stalls_build(&scratch_url).await;
    new_polling_waiter_does_not_stall_build(&scratch_url).await;

    // Whatever is still connected (an aborted waiter's socket can take a
    // moment to actually drop from Postgres's side) must not stop cleanup —
    // FORCE disconnects everyone else first. Best-effort: a stray scratch
    // database on a developer's persistent local Postgres costs nothing but
    // disk, unlike leaving the suite unable to finish.
    let _ = sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP DATABASE IF EXISTS \"{scratch}\" WITH (FORCE)"
    )))
    .execute(&mut admin)
    .await;
}

/// Phase 1: the shape the old `AdvisoryLock::acquire` had. A connection
/// blocked inside `pg_advisory_lock` holds a snapshot open for as long as it
/// waits, so `CREATE INDEX CONCURRENTLY` elsewhere in the database times
/// out behind it.
async fn old_blocking_waiter_stalls_build(scratch_url: &str) {
    const OLD_KEY: i64 = 101;

    let mut holder = connect(scratch_url).await;
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(OLD_KEY)
        .execute(&mut holder)
        .await
        .unwrap_or_else(|e| panic!("acquire old lock: {e}"));

    let waiter_url = scratch_url.to_string();
    let waiter = tokio::spawn(async move {
        let mut conn = connect(&waiter_url).await;
        // The OLD shape: one open statement, blocked, until the lock frees.
        // Freed by the caller below, once it's done asserting.
        let _ = sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(OLD_KEY)
            .execute(&mut conn)
            .await;
    });
    tokio::time::sleep(WAITER_SETTLE).await;

    let mut builder = connect(scratch_url).await;
    exec(&mut builder, SET_STATEMENT_TIMEOUT).await;
    let result = sqlx::query("CREATE INDEX CONCURRENTLY idx_old ON t_old (id)")
        .execute(&mut builder)
        .await;

    assert!(
        result.is_err(),
        "old shape: CREATE INDEX CONCURRENTLY should have timed out behind a blocking \
         waiter's open snapshot, but it succeeded — the repro no longer demonstrates the bug"
    );

    // Unlock BEFORE dealing with the waiter, and wait for it rather than
    // aborting it: closing the client socket does not interrupt a backend
    // parked on a heavyweight lock wait (Postgres only notices a vanished
    // client between statements), so an abort here could leave a zombie
    // backend that wakes up, briefly holds OLD_KEY plus an open snapshot,
    // and only then exits — possibly overlapping the next phase's build.
    // Unlocking first lets the waiter's blocked statement actually
    // complete and its connection close normally. Bounded so a bug here
    // can't hang the test.
    let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(OLD_KEY)
        .execute(&mut holder)
        .await;
    let _ = holder.close().await;
    let _ = tokio::time::timeout(Duration::from_secs(5), waiter).await;
    let _ = builder.close().await;
}

/// Phase 2: the fix. [`AdvisoryLock::acquire`] polls `pg_try_advisory_lock`
/// — every attempt is one short statement, so no snapshot is held open
/// between attempts, and `CREATE INDEX CONCURRENTLY` elsewhere completes
/// normally even while the lock is still held and the waiter is still
/// retrying.
async fn new_polling_waiter_does_not_stall_build(scratch_url: &str) {
    const NEW_KEY: i64 = 202;

    let mut holder = connect(scratch_url).await;
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(NEW_KEY)
        .execute(&mut holder)
        .await
        .unwrap_or_else(|e| panic!("acquire new lock: {e}"));

    let waiter_url = scratch_url.to_string();
    let waiter = tokio::spawn(async move {
        // The actual production code under test. `holder` above doesn't
        // release until after the assertion below, so this polls for a
        // while — then returns once the caller unlocks.
        let _lock = AdvisoryLock::acquire(&waiter_url, NEW_KEY).await;
    });
    tokio::time::sleep(WAITER_SETTLE).await;

    let mut builder = connect(scratch_url).await;
    exec(&mut builder, SET_STATEMENT_TIMEOUT).await;
    let result = sqlx::query("CREATE INDEX CONCURRENTLY idx_new ON t_new (id)")
        .execute(&mut builder)
        .await;

    assert!(
        result.is_ok(),
        "new shape: CREATE INDEX CONCURRENTLY should complete while AdvisoryLock::acquire is \
         still polling for a lock someone else holds, but it errored: {:?}",
        result.err()
    );

    // Same order as phase 1, for the same reason (and here it lets the
    // waiter's poll loop actually observe the unlock and return, rather
    // than being cut off mid-backoff).
    let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(NEW_KEY)
        .execute(&mut holder)
        .await;
    let _ = holder.close().await;
    let _ = tokio::time::timeout(Duration::from_secs(5), waiter).await;
    let _ = builder.close().await;
}
