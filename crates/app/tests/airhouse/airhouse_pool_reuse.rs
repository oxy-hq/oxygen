//! `airhouse_pool` reuses an idle open connection before opening a new one, so
//! an identity holds as many connections as it has had concurrent callers, not
//! the pool's full width. Every open connection holds one of its tenant's
//! connection slots on the Airhouse DP, running a query or not; at width 12,
//! strict round-robin opened all 12 for one caller in a row and filled the
//! analytics DP's connection cap (prod, 2026-10-29).
//!
//! The test Postgres stands in for Airhouse's pgwire endpoint, as in
//! `airhouse_pool_poison`.
//!
//! Run with: `cargo nextest run -p oxy-app --test airhouse -E 'test(airhouse_pool_reuse)'`

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use agentic_connector::DatabaseConnector;
use oxy_app::agentic_wiring::airhouse_pool::get_or_build;

use crate::airhouse_pool_poison::{Target, target};
use crate::common::{Schema, fresh_db};

const WIDTH: usize = 3;

/// Check out the test identity's connector, counting builds.
async fn checkout(target: &Arc<Target>, builds: &Arc<AtomicUsize>) -> Arc<dyn DatabaseConnector> {
    let (target, builds) = (Arc::clone(target), Arc::clone(builds));
    get_or_build("mgd:reuse-test:user:Member".into(), move || async move {
        builds.fetch_add(1, Ordering::SeqCst);
        target.connect().await
    })
    .await
    .expect("a pooled connection")
}

fn same(a: &Arc<dyn DatabaseConnector>, b: &Arc<dyn DatabaseConnector>) -> bool {
    std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b))
}

#[tokio::test]
async fn the_pool_grows_with_concurrent_callers_not_with_checkouts() {
    // SAFETY: nextest runs each test in its own process, and this runs before
    // anything else reads the environment.
    unsafe { std::env::set_var("OXY_AIRHOUSE_POOL_CONNS_PER_IDENTITY", WIDTH.to_string()) };
    let (_db, url) = fresh_db(Schema::CentralAirhouse).await;
    let target = Arc::new(target(&url));
    let builds = Arc::new(AtomicUsize::new(0));

    // One caller at a time: every checkout reuses the first connection.
    // (Round-robin opened all WIDTH of them here.)
    let first = checkout(&target, &builds).await;
    drop(first);
    for _ in 0..2 * WIDTH {
        drop(checkout(&target, &builds).await);
    }
    assert_eq!(
        builds.load(Ordering::SeqCst),
        1,
        "a sequential caller opened more than one connection"
    );

    // Callers holding connections at once: each new one gets its own, up to WIDTH.
    let held: Vec<_> = {
        let mut held = Vec::new();
        for _ in 0..WIDTH {
            held.push(checkout(&target, &builds).await);
        }
        held
    };
    assert_eq!(builds.load(Ordering::SeqCst), WIDTH);
    for (i, a) in held.iter().enumerate() {
        for b in &held[i + 1..] {
            assert!(
                !same(a, b),
                "two concurrent callers were handed one connection while a slot was free"
            );
        }
    }

    // Past WIDTH, a caller shares an open connection rather than opening one.
    let extra = checkout(&target, &builds).await;
    assert_eq!(
        builds.load(Ordering::SeqCst),
        WIDTH,
        "the pool opened past its width"
    );
    assert!(held.iter().any(|h| same(h, &extra)));

    // Once released, the open connections are reused: nothing new is built.
    drop(extra);
    drop(held);
    for _ in 0..WIDTH {
        checkout(&target, &builds).await;
    }
    assert_eq!(builds.load(Ordering::SeqCst), WIDTH);
}
