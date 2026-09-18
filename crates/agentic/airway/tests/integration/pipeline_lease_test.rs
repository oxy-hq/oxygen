//! Integration tests for the airway single-flight lease.
//!
//! These need a real Postgres: the whole point of the lease is that the
//! *database* resolves the race between replicas, so an in-memory fake would
//! test the mock rather than the guarantee. Uses testcontainers (never the dev
//! DB) — set `OXY_DATABASE_URL` to point at a throwaway instance instead.

use agentic_airway::extension::pipeline_lease::{
    LEASE_TTL_SECS, LeaseAcquisition, release_by_run, release_counted, try_acquire,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use uuid::Uuid;

use crate::harness::test_db;

/// Every test uses a fresh workspace uuid, so cases stay independent without
/// truncating a table other tests may be using concurrently.
fn ws() -> Uuid {
    Uuid::new_v4()
}

#[tokio::test]
async fn second_acquire_is_refused_and_names_the_holder() {
    let Some(db) = test_db().await else { return };
    let (w, pipe) = (ws(), "restaurant_analytics");

    let first = try_acquire(&db, w, pipe, "run-a", LEASE_TTL_SECS)
        .await
        .expect("first acquire");
    assert_eq!(first, LeaseAcquisition::Acquired);

    let second = try_acquire(&db, w, pipe, "run-b", LEASE_TTL_SECS)
        .await
        .expect("second acquire");
    // Not merely refused — it must identify the incumbent, which is what the
    // 409 body and the scheduler's skip log both surface.
    match second {
        LeaseAcquisition::Held { run_id, .. } => assert_eq!(run_id, "run-a"),
        LeaseAcquisition::Acquired => panic!("second run must not acquire"),
    }
}

#[tokio::test]
async fn a_different_workspace_is_not_gated_by_the_same_pipeline_name() {
    let Some(db) = test_db().await else { return };
    // `pipeline_name` comes from the YAML and is not globally unique. Two
    // tenants shipping `restaurant_analytics` must not block each other —
    // that would be a cross-tenant denial of service.
    let pipe = "restaurant_analytics";

    assert_eq!(
        try_acquire(&db, ws(), pipe, "run-tenant-1", LEASE_TTL_SECS)
            .await
            .unwrap(),
        LeaseAcquisition::Acquired
    );
    assert_eq!(
        try_acquire(&db, ws(), pipe, "run-tenant-2", LEASE_TTL_SECS)
            .await
            .unwrap(),
        LeaseAcquisition::Acquired
    );
}

#[tokio::test]
async fn release_lets_the_next_run_in() {
    let Some(db) = test_db().await else { return };
    let (w, pipe) = (ws(), "orders");

    try_acquire(&db, w, pipe, "run-a", LEASE_TTL_SECS)
        .await
        .unwrap();
    release_counted(&db, w, pipe, "run-a")
        .await
        .expect("release");

    assert_eq!(
        try_acquire(&db, w, pipe, "run-b", LEASE_TTL_SECS)
            .await
            .unwrap(),
        LeaseAcquisition::Acquired
    );
}

#[tokio::test]
async fn release_by_run_finds_the_lease_without_workspace_or_pipeline() {
    let Some(db) = test_db().await else { return };
    let (w, pipe) = (ws(), "checks");

    try_acquire(&db, w, pipe, "run-a", LEASE_TTL_SECS)
        .await
        .unwrap();
    // This is the path the worker takes at task completion — it holds the run
    // id but not the workspace.
    release_by_run(&db, "run-a").await.expect("release_by_run");

    assert_eq!(
        try_acquire(&db, w, pipe, "run-b", LEASE_TTL_SECS)
            .await
            .unwrap(),
        LeaseAcquisition::Acquired
    );
}

#[tokio::test]
async fn a_stale_release_cannot_free_the_successors_lease() {
    let Some(db) = test_db().await else { return };
    let (w, pipe) = (ws(), "payments");

    // run-a takes a lease that expires immediately, then run-b takes over.
    try_acquire(&db, w, pipe, "run-a", 0).await.unwrap();
    assert_eq!(
        try_acquire(&db, w, pipe, "run-b", LEASE_TTL_SECS)
            .await
            .unwrap(),
        LeaseAcquisition::Acquired
    );

    // run-a finally finishes and releases. Without the run_id guard this would
    // free run-b's live lease and re-admit exactly the concurrency the lease
    // exists to prevent.
    release_counted(&db, w, pipe, "run-a").await.unwrap();
    release_by_run(&db, "run-a").await.unwrap();

    match try_acquire(&db, w, pipe, "run-c", LEASE_TTL_SECS)
        .await
        .unwrap()
    {
        LeaseAcquisition::Held { run_id, .. } => assert_eq!(run_id, "run-b"),
        LeaseAcquisition::Acquired => {
            panic!("run-a's stale release freed run-b's lease")
        }
    }
}

#[tokio::test]
async fn an_expired_lease_is_taken_over() {
    let Some(db) = test_db().await else { return };
    let (w, pipe) = (ws(), "selections");

    // ttl 0 → `expires_at = now()`, already lapsed for the next caller. This
    // is the crashed-worker case: nothing released, but the lease must not
    // block the pipeline forever.
    try_acquire(&db, w, pipe, "crashed-run", 0).await.unwrap();

    assert_eq!(
        try_acquire(&db, w, pipe, "next-run", LEASE_TTL_SECS)
            .await
            .unwrap(),
        LeaseAcquisition::Acquired
    );
}

#[tokio::test]
async fn concurrent_acquirers_produce_exactly_one_winner() {
    let Some(db) = test_db().await else { return };
    let (w, pipe) = (ws(), "concurrent");

    // The guarantee that matters: N replicas racing to start the same pipeline
    // must yield exactly one winner. A check-then-act implementation passes
    // every other test in this file and fails this one.
    let mut set = tokio::task::JoinSet::new();
    for i in 0..8 {
        let db = db.clone();
        let pipe = pipe.to_string();
        set.spawn(async move {
            try_acquire(&db, w, &pipe, &format!("run-{i}"), LEASE_TTL_SECS)
                .await
                .expect("acquire")
        });
    }

    let mut winners = 0;
    while let Some(res) = set.join_next().await {
        if res.expect("task panicked") == LeaseAcquisition::Acquired {
            winners += 1;
        }
    }
    assert_eq!(winners, 1, "expected exactly one winner, got {winners}");
}

#[tokio::test]
async fn lease_row_is_gone_after_release() {
    let Some(db) = test_db().await else { return };
    let (w, pipe) = (ws(), "row_check");

    try_acquire(&db, w, pipe, "run-a", LEASE_TTL_SECS)
        .await
        .unwrap();
    release_by_run(&db, "run-a").await.unwrap();

    // Released means deleted, not tombstoned — a lingering row with a past
    // expiry would still work, but it would grow unboundedly.
    let remaining = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT run_id FROM airway_pipeline_leases WHERE workspace_id = $1",
            [w.into()],
        ))
        .await
        .expect("query");
    assert!(remaining.is_empty(), "lease row should be deleted");
}

/// A run that already holds the lease must be able to re-take it.
///
/// Regression: acquisition moved to claim time, but `retry_airway` and the
/// backfill re-drive still acquire at submit under the ORIGINAL run_id (which
/// is deliberate — the retry is the same run). Without re-entrancy the
/// executor's claim-time `try_acquire` returns `Held { run_id: <itself> }`, the
/// task defers, and it loops every 30s until the 12h ceiling dead-letters it —
/// so retry and backfill re-drive would never execute again.
#[tokio::test]
async fn a_run_can_retake_its_own_lease() {
    let Some(db) = test_db().await else { return };
    let (w, pipe) = (ws(), "reentrant");

    assert!(matches!(
        try_acquire(&db, w, pipe, "run-a", LEASE_TTL_SECS)
            .await
            .unwrap(),
        LeaseAcquisition::Acquired
    ));

    // The same run acquiring again is not contention.
    assert!(
        matches!(
            try_acquire(&db, w, pipe, "run-a", LEASE_TTL_SECS)
                .await
                .unwrap(),
            LeaseAcquisition::Acquired
        ),
        "a run must be able to re-take its own lease, or retry/re-drive deadlocks"
    );

    // And re-entrancy must not weaken the guard against anyone else.
    assert!(
        matches!(
            try_acquire(&db, w, pipe, "run-b", LEASE_TTL_SECS)
                .await
                .unwrap(),
            LeaseAcquisition::Held { .. }
        ),
        "a DIFFERENT run must still be refused"
    );
}
