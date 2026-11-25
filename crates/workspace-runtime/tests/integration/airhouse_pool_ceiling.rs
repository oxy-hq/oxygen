//! `airhouse_pool::ceiling` caps how many Airhouse connections one Oxy process
//! holds open at once, and releases a slot exactly when the connection closes.
//!
//! The pool's per-identity caps never composed into a safe total: before this
//! ceiling they multiplied out to `(16 + 16) x 3 + 8 x 3 = 120` connections
//! from one process, with nothing relating that to the bounded number of
//! connections the shared data plane admits per pod across all tenants — so
//! one Oxy process could exhaust it by itself.
//!
//! `ceiling::tests` pins the arithmetic. This pins the enforcement, and in
//! particular the design claim the arithmetic cannot check: **the permit is
//! held by the connector, not the pool slot**, so dropping the connection is
//! what frees the slot.
//!
//! The test Postgres stands in for Airhouse's pgwire endpoint, as in
//! `airhouse_pool_poison`.
//!
//! Run with: `cargo nextest run -p oxy-workspace-runtime --test integration -E 'test(airhouse_pool_ceiling)'`

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use airhouse::AirhouseConnector;
use oxy_shared::errors::OxyError;
use oxy_workspace_runtime::airhouse_pool::ceiling::connect_within_budget;

use crate::airhouse_pool_poison::{Target, target};
use crate::common::{Schema, fresh_db};
use agentic_connector::DatabaseConnector;
use oxy_workspace_runtime::airhouse_pool::get_or_build;
use std::time::{SystemTime, UNIX_EPOCH};

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock after the epoch")
        .as_secs()
}

/// Open one connection against the process budget, counting attempts.
async fn open(
    target: &Arc<Target>,
    attempts: &Arc<AtomicUsize>,
) -> Result<AirhouseConnector, OxyError> {
    connect_within_budget("mgd:ceiling-test:user:Member", || async {
        attempts.fetch_add(1, Ordering::SeqCst);
        target.open_one().await
    })
    .await
}

#[tokio::test]
async fn the_process_ceiling_bounds_open_connections_and_a_drop_frees_a_slot() {
    // A ceiling of one, so a single held connection is the whole budget.
    // SAFETY: nextest runs each test in its own process, and this runs before
    // anything else reads the environment — the ceiling is read once, when the
    // first connection is opened.
    unsafe { std::env::set_var("OXY_AIRHOUSE_POOL_MAX_LIVE_CONNS", "1") };
    let (_db, url) = fresh_db(Schema::CentralAirhouse).await;
    let target = Arc::new(target(&url));
    let attempts = Arc::new(AtomicUsize::new(0));

    let held = open(&target, &attempts)
        .await
        .expect("the first connection");
    assert_eq!(attempts.load(Ordering::SeqCst), 1);

    // At the ceiling: the next connect waits for a slot and then reports local
    // saturation rather than queueing in Airhouse's admission queue.
    // Not `expect_err`: `AirhouseConnector` is not `Debug`.
    let msg = match open(&target, &attempts).await {
        Ok(_) => panic!("a second connection was opened past a ceiling of one"),
        Err(e) => e.to_string(),
    };
    assert!(
        msg.contains("maximum of 1 airhouse"),
        "the error should name the ceiling it hit, got: {msg}"
    );
    assert!(
        msg.contains("OXY_AIRHOUSE_POOL_MAX_LIVE_CONNS"),
        "the error should name the knob, got: {msg}"
    );
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        1,
        "no connect should have been attempted without a slot"
    );

    // Dropping the connection releases its slot: the permit lives on the
    // connector, so the socket closing is what frees the budget.
    drop(held);
    let after = open(&target, &attempts).await;
    assert!(
        after.is_ok(),
        "the slot was not released when the connection dropped: {:?}",
        after.err()
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

/// Retiring a slot must release its budget permit *before* the replacement is
/// opened. If the retired connector stayed in the slot across the rebuild, one
/// logical slot would need two permits at once — and since credentials minted
/// together lapse together, a wave of retirements would each wait on a permit
/// the others hold and then fail at the ceiling.
///
/// Pinned at a ceiling of one, where a rebuild that needs a second permit
/// cannot possibly succeed.
#[tokio::test]
async fn retiring_a_slot_frees_its_permit_before_the_replacement_is_opened() {
    // SAFETY: nextest runs each test in its own process, and this runs before
    // anything else reads the environment.
    unsafe {
        std::env::set_var("OXY_AIRHOUSE_POOL_MAX_LIVE_CONNS", "1");
        std::env::set_var("OXY_AIRHOUSE_POOL_CONNS_PER_IDENTITY", "1");
    }
    let (_db, url) = fresh_db(Schema::CentralAirhouse).await;
    let target = Arc::new(target(&url));
    let builds = Arc::new(AtomicUsize::new(0));
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock after the epoch")
        .as_secs();

    // Every build goes through the process budget, as the real paths do, and
    // carries a credential already inside the retire buffer — so the pooled
    // connection is one the pool must replace rather than hand out again.
    let spent = now + 1;
    let checkout = || {
        let (target, builds) = (Arc::clone(&target), Arc::clone(&builds));
        async move {
            get_or_build("mgd:ceiling-retire:user:Member".to_string(), move || {
                let (target, builds) = (Arc::clone(&target), Arc::clone(&builds));
                async move {
                    builds.fetch_add(1, Ordering::SeqCst);
                    let conn = connect_within_budget("mgd:ceiling-retire:user:Member", || async {
                        target.open_one().await
                    })
                    .await?;
                    Ok::<_, OxyError>(Arc::new(conn.credential_expires_at(spent)))
                }
            })
            .await
        }
    };

    // First checkout builds, and that connection holds the only permit.
    let first = checkout().await.expect("the first connection");
    assert_eq!(builds.load(Ordering::SeqCst), 1);
    drop(first);

    // Its credential is inside the retire buffer, so the next checkout must
    // retire it, release its permit, and rebuild — all inside a budget of one.
    let rebuilt = checkout().await;
    assert!(
        rebuilt.is_ok(),
        "the retired connection held its permit across the rebuild, so the \
         replacement could not be opened: {:?}",
        rebuilt.err()
    );
    assert_eq!(
        builds.load(Ordering::SeqCst),
        2,
        "the spent credential should have forced exactly one rebuild"
    );

    // And the rebuilt connection actually works. DDL, not `SELECT 1`: the
    // connector routes a `SELECT` through DuckDB-specific introspection the
    // Postgres stand-in cannot serve.
    let conn: Arc<dyn DatabaseConnector> = rebuilt.expect("the rebuilt connection");
    assert!(
        conn.execute_statement("DROP TABLE IF EXISTS _airhouse_pool_ceiling_probe")
            .await
            .is_ok()
    );
}

/// Open one connection for `key` through the process budget, carrying
/// `deadline` as its credential's expiry.
async fn checkout_key(
    key: &str,
    deadline: u64,
    target: &Arc<Target>,
    builds: &Arc<AtomicUsize>,
) -> Result<Arc<dyn DatabaseConnector>, OxyError> {
    let (target, builds, owned) = (Arc::clone(target), Arc::clone(builds), key.to_string());
    get_or_build(owned.clone(), move || async move {
        builds.fetch_add(1, Ordering::SeqCst);
        let conn = connect_within_budget(&owned, || async { target.open_one().await }).await?;
        Ok(Arc::new(conn.credential_expires_at(deadline)))
    })
    .await
}

/// A pooled connection keeps its budget permit until its slot is reused or its
/// identity evicted, so without reclamation the ceiling refuses a *new*
/// identity while the process is completely idle. That is the regression this
/// pins: at a ceiling of one, a second identity must be able to take the
/// permit an idle first identity is sitting on.
#[tokio::test]
async fn an_idle_connection_from_another_identity_gives_up_its_permit() {
    // SAFETY: nextest runs each test in its own process, and this runs before
    // anything else reads the environment.
    unsafe {
        std::env::set_var("OXY_AIRHOUSE_POOL_MAX_LIVE_CONNS", "1");
        std::env::set_var("OXY_AIRHOUSE_POOL_CONNS_PER_IDENTITY", "1");
    }
    let (_db, url) = fresh_db(Schema::CentralAirhouse).await;
    let target = Arc::new(target(&url));
    let builds = Arc::new(AtomicUsize::new(0));
    let far = now_secs() + 86_400;

    // Identity A takes the only permit, then goes idle in its slot — alive and
    // perfectly reusable, which is why nothing else would reclaim it.
    let a = checkout_key("mgd:ws-a:user:Member", far, &target, &builds)
        .await
        .expect("identity A");
    drop(a);
    assert_eq!(builds.load(Ordering::SeqCst), 1);

    // Identity B, with nothing running anywhere.
    let b = checkout_key("mgd:ws-b:user:Member", far, &target, &builds).await;
    assert!(
        b.is_ok(),
        "an idle connection held the whole budget while the process was idle: {:?}",
        b.err()
    );
    assert_eq!(builds.load(Ordering::SeqCst), 2);
    assert!(
        b.expect("identity B")
            .execute_statement("DROP TABLE IF EXISTS _airhouse_pool_idle_probe")
            .await
            .is_ok()
    );
}

/// The same for a connection that can never serve again. A credential-spent or
/// closed socket is only noticed when its own slot is next checked out, so
/// after a data-plane restart every dead slot would go on holding a permit
/// until the idle sweep ran — locking the process out of the data plane it had
/// just reconnected to.
#[tokio::test]
async fn a_spent_connection_from_another_identity_stops_holding_the_budget() {
    // SAFETY: as above.
    unsafe {
        std::env::set_var("OXY_AIRHOUSE_POOL_MAX_LIVE_CONNS", "1");
        std::env::set_var("OXY_AIRHOUSE_POOL_CONNS_PER_IDENTITY", "1");
    }
    let (_db, url) = fresh_db(Schema::CentralAirhouse).await;
    let target = Arc::new(target(&url));
    let builds = Arc::new(AtomicUsize::new(0));

    // Identity A's credential is already inside the retire buffer, so its
    // connection is unusable — but it is A's slot that holds the permit, and
    // nothing checks A out again.
    let spent = now_secs() + 1;
    let a = checkout_key("mgd:ws-a:user:Member", spent, &target, &builds)
        .await
        .expect("identity A");
    drop(a);

    let b = checkout_key(
        "mgd:ws-b:user:Member",
        now_secs() + 86_400,
        &target,
        &builds,
    )
    .await;
    assert!(
        b.is_ok(),
        "a connection that can never serve again held the whole budget: {:?}",
        b.err()
    );
    assert_eq!(builds.load(Ordering::SeqCst), 2);
}

/// A permit is **not** returned when the request borrowing a pooled connection
/// finishes: the borrower's `Arc` drops and the connector goes back to being
/// idle in its slot, still holding the permit. So reclaiming once before the
/// wait is not enough — a burst that is in flight when a waiter arrives frees
/// nothing on that first pass, and the waiter would be refused while every
/// connection sat idle.
///
/// At a ceiling of one: hold A's connection, start B's checkout, release A
/// shortly after, and B must get through well inside `PERMIT_WAIT` (3s).
#[tokio::test]
async fn a_connection_that_falls_idle_during_the_wait_is_reclaimed() {
    // SAFETY: nextest runs each test in its own process, and this runs before
    // anything else reads the environment.
    unsafe {
        std::env::set_var("OXY_AIRHOUSE_POOL_MAX_LIVE_CONNS", "1");
        std::env::set_var("OXY_AIRHOUSE_POOL_CONNS_PER_IDENTITY", "1");
    }
    let (_db, url) = fresh_db(Schema::CentralAirhouse).await;
    let target = Arc::new(target(&url));
    let builds = Arc::new(AtomicUsize::new(0));
    let far = now_secs() + 86_400;

    // A is checked out and still *borrowed*, so it is not reclaimable yet.
    let a = checkout_key("mgd:ws-a:user:Member", far, &target, &builds)
        .await
        .expect("identity A");

    // B starts waiting while A is in flight: the first reclaim pass finds
    // nothing, exactly as it would during a real burst.
    let (bt, bb) = (Arc::clone(&target), Arc::clone(&builds));
    let b = tokio::spawn(async move { checkout_key("mgd:ws-b:user:Member", far, &bt, &bb).await });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // A's request finishes. This does NOT hand back the permit — the connector
    // is now idle in A's slot, holding it.
    drop(a);

    let started = std::time::Instant::now();
    let got = b.await.expect("the waiting checkout did not panic");
    assert!(
        got.is_ok(),
        "a connection that fell idle during the wait was never reclaimed: {:?}",
        got.err()
    );
    // Tight on purpose. The loop reclaims once more after its wait expires, so
    // a coarse tick still lets B through — but only at the very end of
    // `PERMIT_WAIT`, which is the latency this finding is about. A reclaim on
    // the tick frees it in roughly one tick, so anything near the full wait
    // means the retick is not doing the work.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "B only got through after {:?}, near the full PERMIT_WAIT — the reclaim \
         tick is not what freed it",
        started.elapsed()
    );
    assert_eq!(builds.load(Ordering::SeqCst), 2);
}
