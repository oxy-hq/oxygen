//! The ledger of owed purges: it outlives the queue, backs off, gives up once,
//! and never requeues a purge that succeeded. Time is passed in, never slept.

use chrono::{DateTime, Duration, Utc};
use entity::custom_app_usage_purges as purges;
use oxy_api_custom_apps::admin::apps::handlers::delete_app;
use oxy_api_custom_apps::custom_apps_usage_purge::ledger::{self, Failure, MAX_ATTEMPTS};
use oxy_api_custom_apps::custom_apps_usage_purge::{attempt, requeue_owed_at};
use oxy_app_core::agent_refusal::RefuseSandboxAgent;
use sea_orm::{ConnectionTrait, DatabaseBackend, EntityTrait, Statement};

use super::*;

impl World {
    /// Delete app `i` and return when, so a test can step the clock from there.
    async fn delete(&self, i: usize) -> DateTime<Utc> {
        delete_app(RefuseSandboxAgent, self.actor.clone(), Path(self.apps[i]))
            .await
            .expect("delete");
        Utc::now()
    }

    pub(super) async fn row(&self, i: usize) -> purges::Model {
        purges::Entity::find_by_id(self.apps[i])
            .one(&self.db)
            .await
            .expect("read the ledger")
            .expect("the delete wrote a ledger row")
    }

    /// The queue row the delete (or a sweep) left is over, whichever way.
    async fn finish_queued_run(&self, i: usize) {
        self.set_status(self.apps[i], "failed").await;
    }

    async fn forget_queue(&self, i: usize) {
        self.db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "DELETE FROM agentic_task_queue WHERE spec->'payload'->>'app_id' = $1",
                [self.apps[i].to_string().into()],
            ))
            .await
            .expect("prune the queue");
    }
}

#[tokio::test]
async fn an_outage_past_the_old_retry_window_leaves_the_purge_pending() {
    let w = world().await;
    let t0 = w.delete(0).await;
    w.fake.fail_deletes(true);

    // One sweep every five minutes for a day: the old schedule spent all its
    // 20 requeues in the first 100 minutes of this.
    let mut now = t0;
    assert!(attempt(&w.db, &w.task(0), now).await.is_err());
    w.finish_queued_run(0).await;
    for _ in 0..288 {
        now += Duration::minutes(5);
        if requeue_owed_at(&w.db, now).await.unwrap() == 1 {
            assert!(attempt(&w.db, &w.task(0), now).await.is_err());
            w.finish_queued_run(0).await;
        }
    }

    let row = w.row(0).await;
    assert!(row.abandoned_at.is_none() && row.completed_at.is_none());
    assert!(
        (5..20).contains(&row.attempts),
        "a day of outage is {} attempts",
        row.attempts
    );
    assert!(row.next_attempt_at.with_timezone(&Utc) > now);
    assert_eq!(w.facts_of(0), 1);

    // Airhouse comes back: the next due run deletes the facts.
    w.fake.fail_deletes(false);
    now = row.next_attempt_at.with_timezone(&Utc);
    assert_eq!(requeue_owed_at(&w.db, now).await.unwrap(), 1);
    attempt(&w.db, &w.task(0), now)
        .await
        .expect("the purge runs");
    assert_eq!(w.facts_of(0), 0);
    assert!(w.row(0).await.completed_at.is_some());
}

#[tokio::test]
async fn a_purge_is_abandoned_once_and_never_queued_after() {
    let w = world().await;
    let t0 = w.delete(0).await;
    w.fake.fail_deletes(true);
    w.finish_queued_run(0).await;

    let mut outcomes = Vec::new();
    for i in 1..=MAX_ATTEMPTS {
        let now = t0 + Duration::hours(7 * i64::from(i));
        assert_eq!(requeue_owed_at(&w.db, now).await.unwrap(), 1, "run {i}");
        outcomes.push(ledger::fail(&w.db, w.apps[0], "down", now).await.unwrap());
        w.finish_queued_run(0).await;
    }
    let abandoned = outcomes
        .iter()
        .filter(|o| matches!(o, Failure::Abandoned { .. }))
        .count();
    assert_eq!(abandoned, 1, "the give-up is reported exactly once");
    assert!(matches!(outcomes.last(), Some(Failure::Abandoned { .. })));
    assert!(
        outcomes[..outcomes.len() - 1]
            .iter()
            .all(|o| matches!(o, Failure::Retry { .. }))
    );

    let row = w.row(0).await;
    assert!(row.abandoned_at.is_some() && row.completed_at.is_none());
    assert_eq!(row.attempts, MAX_ATTEMPTS);
    let runs = w.purges(w.apps[0]).await.len();

    let later = t0 + Duration::days(40);
    assert_eq!(requeue_owed_at(&w.db, later).await.unwrap(), 0);
    assert_eq!(w.purges(w.apps[0]).await.len(), runs, "nothing was queued");
    assert_eq!(
        ledger::fail(&w.db, w.apps[0], "down", later).await.unwrap(),
        Failure::Settled,
        "a later failure neither counts nor reports again"
    );
    assert_eq!(w.row(0).await.attempts, MAX_ATTEMPTS);
    assert_eq!(w.facts_of(0), 1, "the debt is still visible in Airhouse");
}

#[tokio::test]
async fn a_purge_that_succeeded_after_failures_is_not_requeued_once_the_queue_is_pruned() {
    let w = world().await;
    let t0 = w.delete(0).await;
    w.fake.fail_deletes(true);
    assert!(attempt(&w.db, &w.task(0), t0).await.is_err());
    w.finish_queued_run(0).await;
    let t1 = t0 + Duration::hours(1);
    assert_eq!(requeue_owed_at(&w.db, t1).await.unwrap(), 1);
    assert!(attempt(&w.db, &w.task(0), t1).await.is_err());
    w.finish_queued_run(0).await;

    w.fake.fail_deletes(false);
    let t2 = t1 + Duration::hours(1);
    assert_eq!(requeue_owed_at(&w.db, t2).await.unwrap(), 1);
    attempt(&w.db, &w.task(0), t2)
        .await
        .expect("the purge runs");
    assert_eq!(w.facts_of(0), 0);

    // Queue retention has since removed every row of the purge's runs.
    w.forget_queue(0).await;
    assert!(w.purges(w.apps[0]).await.is_empty());
    let t3 = t2 + Duration::days(30);
    assert_eq!(requeue_owed_at(&w.db, t3).await.unwrap(), 0);
    assert!(w.purges(w.apps[0]).await.is_empty());
    assert!(w.row(0).await.completed_at.is_some(), "kept as the record");

    // Completed rows are pruned after 90 days; owed ones are not.
    let (other, t4) = (w.delete(1).await, t3 + Duration::days(91));
    let _ = other;
    requeue_owed_at(&w.db, t4).await.unwrap();
    assert!(
        purges::Entity::find_by_id(w.apps[0])
            .one(&w.db)
            .await
            .unwrap()
            .is_none()
    );
    assert!(w.row(1).await.completed_at.is_none(), "an owed row stays");
}
