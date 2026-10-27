use chrono::Duration;
use oxy_app::server::experiment::jobs::{self, CHECK_KIND, ESTIMATE_KIND};

use super::fixture::*;

#[tokio::test]
async fn the_estimate_is_enqueued_once_per_run() {
    let fx = Fx::seed(DESIGN).await;
    let run = fx.register().await;
    assert!(jobs::enqueue_estimate(&fx.db, &run).await.unwrap());
    let run = fx.run().await;
    assert!(run.estimate_task_id.is_some());
    assert!(!jobs::enqueue_estimate(&fx.db, &run).await.unwrap());
    assert_eq!(fx.queued_kind(ESTIMATE_KIND).await, 1);
}

#[tokio::test]
async fn two_replicas_enqueueing_the_estimate_at_once_enqueue_one() {
    let fx = Fx::seed(DESIGN).await;
    let run = fx.register().await;
    let (a, b) = tokio::join!(
        jobs::enqueue_estimate(&fx.db, &run),
        jobs::enqueue_estimate(&fx.db, &run)
    );
    assert_eq!(u8::from(a.unwrap()) + u8::from(b.unwrap()), 1);
    assert_eq!(fx.queued_kind(ESTIMATE_KIND).await, 1);
}

#[tokio::test]
async fn the_check_is_enqueued_at_most_hourly() {
    let fx = Fx::seed(DESIGN).await;
    let t0 = utc("2026-03-02T00:00:00Z");
    let run = fx.register().await;
    assert!(jobs::maybe_enqueue_check(&fx.db, &run, t0).await.unwrap());
    assert!(
        !jobs::maybe_enqueue_check(&fx.db, &run, t0 + Duration::minutes(30))
            .await
            .unwrap()
    );
    assert!(
        jobs::maybe_enqueue_check(&fx.db, &run, t0 + Duration::minutes(60))
            .await
            .unwrap()
    );
    assert_eq!(fx.queued_kind(CHECK_KIND).await, 2);
}
