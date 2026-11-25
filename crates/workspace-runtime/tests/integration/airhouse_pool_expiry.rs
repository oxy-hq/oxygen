//! `airhouse_pool` retires a pooled connection before its ephemeral credential
//! expires, instead of waiting for Airhouse to close the socket.
//!
//! Airhouse closes a session whose credential has lapsed — deliberately, since
//! a SQLSTATE does not evict libpq-family drivers. The pool's liveness check
//! would eventually notice, but only *after* the close, which puts a fresh
//! connect + SCRAM + schema fetch on a request's critical path, through an
//! admission queue with a bounded number of slots per pod. Credentials are
//! minted at `airhouse::DEFAULT_INTERNAL_TTL` (900s), so every busy identity
//! used to do this every ~15 minutes.
//!
//! What makes this test the point rather than a restatement of
//! `airhouse_pool_poison`: the connection being retired here is **still fully
//! alive** — it answers a query immediately before the pool replaces it.
//!
//! The test Postgres stands in for Airhouse's pgwire endpoint, as in
//! `airhouse_pool_poison`.
//!
//! Run with: `cargo nextest run -p oxy-workspace-runtime --test integration -E 'test(airhouse_pool_expiry)'`

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agentic_connector::DatabaseConnector;
use oxy_workspace_runtime::airhouse_pool::get_or_build;

use crate::airhouse_pool_poison::{Target, target};
use crate::common::{Schema, fresh_db};

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock after the epoch")
        .as_secs()
}

/// Check out `key`, counting builds, with every built connection carrying
/// `deadline` as its credential's expiry.
async fn checkout(
    key: &str,
    target: &Arc<Target>,
    builds: &Arc<AtomicUsize>,
    deadline: u64,
) -> Arc<dyn DatabaseConnector> {
    let (target, builds) = (Arc::clone(target), Arc::clone(builds));
    get_or_build(key.to_string(), move || async move {
        builds.fetch_add(1, Ordering::SeqCst);
        target.connect_expiring_at(deadline).await
    })
    .await
    .expect("a pooled connection")
}

/// Proof the connection works right now — so a rebuild is the pool's choice,
/// not a dead socket.
///
/// Deliberately a DDL statement and not `SELECT 1`. The connector classifies a
/// `SELECT` as subquery DQL and answers it through DuckDB-specific
/// introspection (`DESCRIBE (…)`, `(…) AS _q`), which the Postgres stand-in
/// cannot serve — a `SELECT` probe fails here on a perfectly healthy
/// connection. DDL goes straight to `simple_query`, which both engines accept.
/// Idempotent, and it touches no session state, so a shared connection would
/// not refuse it either.
async fn is_alive(conn: &Arc<dyn DatabaseConnector>) -> bool {
    conn.execute_statement("DROP TABLE IF EXISTS _airhouse_pool_liveness_probe")
        .await
        .is_ok()
}

#[tokio::test]
async fn a_live_connection_is_retired_before_its_credential_expires() {
    // One connection per identity, so every checkout lands on the same slot.
    // SAFETY: nextest runs each test in its own process, and this runs before
    // anything else reads the environment.
    unsafe { std::env::set_var("OXY_AIRHOUSE_POOL_CONNS_PER_IDENTITY", "1") };
    let (_db, url) = fresh_db(Schema::CentralAirhouse).await;
    let target = Arc::new(target(&url));

    // A credential good for another day: the pool reuses its connection, which
    // is the behaviour the expiry check must not disturb.
    let fresh_builds = Arc::new(AtomicUsize::new(0));
    let key = "mgd:expiry-test:user:Member:fresh";
    let far = now_secs() + 86_400;
    let first = checkout(key, &target, &fresh_builds, far).await;
    assert!(is_alive(&first).await);
    drop(first);
    for _ in 0..3 {
        drop(checkout(key, &target, &fresh_builds, far).await);
    }
    assert_eq!(
        fresh_builds.load(Ordering::SeqCst),
        1,
        "a credential with a day left was retired early"
    );

    // A credential inside the retire buffer but NOT yet expired: Airhouse
    // would still accept it and the socket is open, and the pool replaces the
    // connection anyway rather than let a request discover the close.
    let spent_builds = Arc::new(AtomicUsize::new(0));
    let key = "mgd:expiry-test:user:Member:spent";
    let soon = now_secs() + 10;
    let expiring = checkout(key, &target, &spent_builds, soon).await;
    assert!(
        is_alive(&expiring).await,
        "the connection about to be retired is alive and serving"
    );
    assert!(
        soon > now_secs(),
        "the credential has not actually expired yet"
    );
    drop(expiring);

    let replaced = checkout(key, &target, &spent_builds, soon).await;
    assert_eq!(
        spent_builds.load(Ordering::SeqCst),
        2,
        "a live connection whose credential was about to lapse was handed out again"
    );
    assert!(is_alive(&replaced).await);
}

#[tokio::test]
async fn a_credential_with_no_expiry_is_pooled_for_as_long_as_it_lives() {
    // Static airhouse: the password is configured, not minted, so there is no
    // deadline and the connection is reused indefinitely. Regression guard for
    // treating "no deadline" as "expired".
    // SAFETY: as above.
    unsafe { std::env::set_var("OXY_AIRHOUSE_POOL_CONNS_PER_IDENTITY", "1") };
    let (_db, url) = fresh_db(Schema::CentralAirhouse).await;
    let target = Arc::new(target(&url));
    let builds = Arc::new(AtomicUsize::new(0));

    let key = "static:expiry-test:airhouse".to_string();
    for _ in 0..4 {
        let (t, b) = (Arc::clone(&target), Arc::clone(&builds));
        let conn = get_or_build(key.clone(), move || async move {
            b.fetch_add(1, Ordering::SeqCst);
            // No `credential_expires_at`: nothing mints this one.
            t.connect().await
        })
        .await
        .expect("a pooled connection");
        assert!(is_alive(&conn).await);
    }
    assert_eq!(
        builds.load(Ordering::SeqCst),
        1,
        "a connection with no credential deadline was rebuilt"
    );
}
