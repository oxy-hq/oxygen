//! `airhouse_pool::ceiling` caps how many Airhouse connections one Oxy process
//! holds open at once, and releases a slot exactly when the connection closes.
//!
//! The pool's per-identity caps never composed into a safe total: at the
//! defaults they multiply out to `(16 + 16) x 3 + 8 x 3 = 120` connections from
//! one process, against the `32 per pod x 2 replicas = 64` the shared data
//! plane offers in total. One Oxy process could exhaust Airhouse by itself,
//! which is what refused 10,977 connections at admission over 2026-11-13..15
//! while CPU sat at 0%.
//!
//! `ceiling::tests` pins the arithmetic. This pins the enforcement, and in
//! particular the design claim the arithmetic cannot check: **the permit is
//! held by the connector, not the pool slot**, so dropping the connection is
//! what frees the slot.
//!
//! The test Postgres stands in for Airhouse's pgwire endpoint, as in
//! `airhouse_pool_poison`.
//!
//! Run with: `cargo nextest run -p oxy-app --test airhouse -E 'test(airhouse_pool_ceiling)'`

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use airhouse::AirhouseConnector;
use oxy_app::agentic_wiring::airhouse_pool::ceiling::connect_within_budget;
use oxy_shared::errors::OxyError;

use crate::airhouse_pool_poison::{Target, target};
use crate::common::{Schema, fresh_db};

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
    // saturation rather than queueing in Airhouse's 30s admission queue.
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
