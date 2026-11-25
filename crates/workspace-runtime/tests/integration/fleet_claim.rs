//! The fleet-job claim (`oxy_app_core::fleet_claim`), against a real Postgres.
//!
//! Every `oxy worker` replica starts the fleet jobs (the storage sweeper, the
//! usage-backfill sweep, the bucket lifecycle check, the audit anchor) and this
//! upsert is all that keeps them from running once per replica. What is worth
//! a database here is what two replicas must never do: both win one interval,
//! or both run the once-per-build check for one build. A claim is made due by
//! moving its `next_run_at` into the past, which is what the passage of an
//! interval does.
//!
//! Run with:
//! `cargo nextest run -p oxy-workspace-runtime --test integration -E 'test(fleet_claim)'`

use std::time::Duration;

use oxy_app_core::fleet_claim::{claim_interval, claim_once};
use sea_orm::{ConnectionTrait, DatabaseConnection};

use crate::common::{Schema, test_db_with};

const INTERVAL: Duration = Duration::from_secs(15 * 60);
const REPLICAS: usize = 8;

async fn an_interval_passes(db: &DatabaseConnection, job: &str) {
    db.execute_unprepared(&format!(
        "UPDATE fleet_job_claims SET next_run_at = now() - interval '1 second' WHERE job = '{job}'"
    ))
    .await
    .expect("make the claim due");
}

/// `REPLICAS` claims in the same instant; how many won.
async fn race_interval(db: &DatabaseConnection, job: &'static str) -> usize {
    let racers = (0..REPLICAS).map(|_| {
        let db = db.clone();
        tokio::spawn(async move { claim_interval(&db, job, INTERVAL).await.expect("claim") })
    });
    let mut won = 0;
    for r in racers {
        won += usize::from(r.await.expect("join"));
    }
    won
}

#[tokio::test]
async fn fleet_claim_is_won_once_per_interval_across_concurrent_replicas() {
    let db = test_db_with(Schema::Central).await;
    let job = "custom_app_storage_sweeper";

    // No row yet: due, and exactly one of the racers creates it.
    assert_eq!(race_interval(&db, job).await, 1, "first interval");
    // Inside the interval nobody wins, however often they ask.
    assert_eq!(race_interval(&db, job).await, 0, "same interval");

    an_interval_passes(&db, job).await;
    assert_eq!(race_interval(&db, job).await, 1, "next interval");
    assert_eq!(race_interval(&db, job).await, 0, "same interval again");

    // Jobs do not share a claim.
    assert!(
        claim_interval(&db, "custom_app_usage_backfill_sweep", INTERVAL)
            .await
            .expect("claim")
    );
}

#[tokio::test]
async fn fleet_claim_once_runs_once_per_key() {
    let db = test_db_with(Schema::Central).await;
    let job = "custom_app_asset_lifecycle_verify";

    let race = |key: &'static str| {
        let racers: Vec<_> = (0..REPLICAS)
            .map(|_| {
                let db = db.clone();
                tokio::spawn(async move { claim_once(&db, job, key).await.expect("claim") })
            })
            .collect();
        async move {
            let mut won = 0;
            for r in racers {
                won += usize::from(r.await.expect("join"));
            }
            won
        }
    };

    assert_eq!(race("0.5.200+abc").await, 1, "a build's first boot");
    assert_eq!(
        race("0.5.200+abc").await,
        0,
        "the same build's other replicas"
    );
    assert_eq!(race("0.5.201+def").await, 1, "the next build");
}
