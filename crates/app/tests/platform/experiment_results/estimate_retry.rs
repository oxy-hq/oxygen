//! A failed estimate through PR 4's tick: released and re-enqueued after the
//! backoff, at most `MAX_ESTIMATE_ATTEMPTS` times, with one owner notice when the
//! attempts run out; a failed job whose result was written anyway is re-run past
//! that cap (the re-run only returns `Done`); and an aborted run, which is
//! estimated too.

use std::sync::Arc;

use agentic_core::delegation::TaskSpec;
use chrono::{DateTime, Days, Duration, Utc};
use entity::experiment_results;
use oxy_app::server::experiment::jobs::{
    self, ESTIMATE_KIND, ESTIMATE_RETRY_AFTER_SECS, MAX_ESTIMATE_ATTEMPTS,
};
use oxy_app::server::experiment::schedule::RUN_CHECK_INTERVAL_SECS;
use oxy_experiment::timeline::{day_start_utc, last_post_window_end_date};
use sea_orm::{
    ColumnTrait, DatabaseBackend, EntityTrait, FromQueryResult, PaginatorTrait, QueryFilter,
    Statement,
};
use serde_json::json;
use uuid::Uuid;

use super::estimate_tick::StorePanels;
use super::harness::{execute, is_done, result_of};
use crate::experiment_runner::fixture::{DESIGN, Fx, switch_at};

const EXHAUSTED: &str = "the estimate failed 3 times";

fn backoff() -> Duration {
    Duration::seconds(ESTIMATE_RETRY_AFTER_SECS)
}

/// A run in `closing` with its first estimate enqueued; returns the tick's instant.
async fn closing_with_estimate() -> (Fx, DateTime<Utc>) {
    let fx = Fx::seed(DESIGN).await;
    fx.register().await;
    fx.tick(switch_at("2026-03-02")).await;
    fx.pin(&["glebe", "paddington"], None).await;
    let end = last_post_window_end_date(&fx.design().await).expect("an end");
    fx.exec(
        "UPDATE experiment_runs SET state = 'closing', closing_since = $1, observations = $2 \
         WHERE workspace_id = $3",
        [end.into(), json!({ "coverage": 0.95 }).into(), fx.ws.into()],
    )
    .await;
    let at = day_start_utc(end + Days::new(1), chrono_tz::Australia::Sydney);
    fx.tick(at).await;
    assert!(
        fx.run().await.estimate_task_id.is_some(),
        "closing enqueued it"
    );
    (fx, at)
}

/// A live run aborted on request, ticked into `aborted`; returns the tick's instant.
async fn aborted() -> (Fx, DateTime<Utc>) {
    let fx = Fx::seed(DESIGN).await;
    fx.register().await;
    let live = switch_at("2026-03-02");
    fx.tick(live).await;
    fx.pin(&["glebe", "paddington"], None).await;
    fx.finish_levers("done").await;
    fx.exec(
        "UPDATE experiment_runs SET abort_reason = 'owner call' WHERE workspace_id = $1",
        [fx.ws.into()],
    )
    .await;
    let at = live + Duration::hours(1);
    fx.tick(at).await;
    assert_eq!(fx.run().await.state, "aborted");
    (fx, at)
}

/// The current estimate task ends `failed` at `at`, as the worker would record it.
async fn fail(fx: &Fx, at: DateTime<Utc>) -> Uuid {
    let task = fx.run().await.estimate_task_id.expect("an estimate task");
    fx.exec(
        "UPDATE agentic_runs SET task_status = 'failed', error_message = 'warehouse down', \
         updated_at = $2 WHERE id = $1",
        [task.to_string().into(), at.fixed_offset().into()],
    )
    .await;
    task
}

async fn exhaustion_notices(fx: &Fx) -> usize {
    fx.notices()
        .await
        .iter()
        .filter(|n| n.body.as_deref().is_some_and(|b| b.contains(EXHAUSTED)))
        .count()
}

async fn estimate_json(fx: &Fx, at: DateTime<Utc>) -> serde_json::Value {
    serde_json::to_value(fx.status_at(at).await.run.expect("a run").estimate).unwrap()
}

#[derive(FromQueryResult)]
struct Queued {
    spec: serde_json::Value,
}

/// Run the executor on `task` exactly as it was queued.
async fn run_queued(fx: &Fx, task: Uuid) {
    let queued = Queued::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT spec FROM agentic_task_queue WHERE task_id = $1",
        [task.to_string().into()],
    ))
    .one(&fx.db)
    .await
    .unwrap()
    .expect("a queued task");
    let spec: TaskSpec = serde_json::from_value(queued.spec).expect("a task spec");
    let outcome = execute(&fx.db, &task.to_string(), spec, Arc::new(StorePanels)).await;
    assert!(is_done(&outcome), "{outcome:?}");
}

#[tokio::test]
async fn a_failed_estimate_is_re_enqueued_only_after_the_backoff() {
    let (fx, at) = closing_with_estimate().await;
    let failed_at = at + Duration::minutes(10);
    let first = fail(&fx, failed_at).await;

    fx.tick(failed_at + backoff() - Duration::minutes(1)).await;
    assert_eq!(
        fx.run().await.estimate_task_id,
        Some(first),
        "inside the backoff"
    );
    assert_eq!(fx.queued_kind(ESTIMATE_KIND).await, 1);

    let retry_at = failed_at + backoff();
    fx.tick(retry_at).await;
    let second = fx.run().await.estimate_task_id.expect("re-enqueued");
    assert_ne!(second, first);
    assert_eq!(fx.queued_kind(ESTIMATE_KIND).await, 2);
    assert_eq!(fx.run().await.state, "closing");
    assert_eq!(exhaustion_notices(&fx).await, 0);
    assert_eq!(estimate_json(&fx, retry_at).await["state"], "running");
}

#[tokio::test]
async fn a_failed_estimate_is_released_once_however_many_replicas_try() {
    let (fx, at) = closing_with_estimate().await;
    let run = fx.run().await;
    let first = fail(&fx, at).await;
    let (a, b) = tokio::join!(
        jobs::release_failed_estimate(&fx.db, run.id, first),
        jobs::release_failed_estimate(&fx.db, run.id, first)
    );
    assert!(a.unwrap() ^ b.unwrap(), "exactly one release wins");

    fx.tick(at + backoff() * 2).await;
    let second = fx.run().await.estimate_task_id.expect("re-enqueued");
    assert_eq!(fx.queued_kind(ESTIMATE_KIND).await, 2);
    // A replica still holding the old failed id releases nothing.
    assert!(
        !jobs::release_failed_estimate(&fx.db, run.id, first)
            .await
            .unwrap()
    );
    fx.tick(at + backoff() * 2 + Duration::minutes(5)).await;
    assert_eq!(fx.run().await.estimate_task_id, Some(second));
    assert_eq!(fx.queued_kind(ESTIMATE_KIND).await, 2);
}

#[tokio::test]
async fn after_the_last_attempt_fails_nothing_is_retried_and_the_owner_is_told_once() {
    let (fx, mut at) = closing_with_estimate().await;
    for attempt in 1..=MAX_ESTIMATE_ATTEMPTS {
        assert_eq!(fx.queued_kind(ESTIMATE_KIND).await, attempt);
        fail(&fx, at).await;
        at += backoff() + Duration::minutes(1);
        fx.tick(at).await;
    }
    let last = fx.run().await.estimate_task_id;
    for later in [Duration::hours(2), Duration::days(1), Duration::days(3)] {
        fx.tick(at + later).await;
    }
    assert_eq!(
        fx.run().await.estimate_task_id,
        last,
        "never released again"
    );
    assert_eq!(fx.queued_kind(ESTIMATE_KIND).await, MAX_ESTIMATE_ATTEMPTS);
    assert_eq!(fx.run().await.state, "closing");
    assert_eq!(exhaustion_notices(&fx).await, 1);
    let notice = fx.notices().await;
    let body = notice
        .iter()
        .find_map(|n| n.body.as_deref().filter(|b| b.contains(EXHAUSTED)))
        .unwrap();
    assert!(
        body.contains("last: failed: warehouse down") && body.contains("stays in closing"),
        "{body}"
    );
    let estimate = estimate_json(&fx, at).await;
    assert_eq!(estimate["state"], "failed");
    assert_eq!(estimate["attempts"], MAX_ESTIMATE_ATTEMPTS);
}

/// The worker's record of `task`: `done`, as it would write the executor's Done.
async fn mark_done(fx: &Fx, task: Uuid) {
    fx.exec(
        "UPDATE agentic_runs SET task_status = 'done' WHERE id = $1",
        [task.to_string().into()],
    )
    .await;
}

async fn result_rows(fx: &Fx, run: Uuid) -> u64 {
    experiment_results::Entity::find()
        .filter(experiment_results::Column::RunId.eq(run))
        .count(&fx.db)
        .await
        .unwrap()
}

/// A result was written but its job reads failed (a lost `Done`): the tick runs
/// it again after the backoff, even with every attempt used, and the re-run only
/// returns `Done`. No exhaustion notice: nothing is missing.
#[tokio::test]
async fn a_failed_estimate_whose_result_exists_is_re_run_past_the_cap_and_the_run_reports() {
    let (fx, mut at) = closing_with_estimate().await;
    let run = fx.run().await;
    for _ in 1..MAX_ESTIMATE_ATTEMPTS {
        fail(&fx, at).await;
        at += backoff() + Duration::minutes(1);
        fx.tick(at).await;
    }
    let last = fx.run().await.estimate_task_id.expect("the last attempt");
    run_queued(&fx, last).await;
    assert_eq!(result_rows(&fx, run.id).await, 1);
    fail(&fx, at).await; // the Done was lost
    assert_eq!(fx.queued_kind(ESTIMATE_KIND).await, MAX_ESTIMATE_ATTEMPTS);

    fx.tick(at + backoff() - Duration::minutes(1)).await;
    assert_eq!(
        fx.run().await.estimate_task_id,
        Some(last),
        "the backoff still holds"
    );
    at += backoff();
    fx.tick(at).await;
    let rerun = fx.run().await.estimate_task_id.expect("re-enqueued");
    assert_ne!(rerun, last);
    run_queued(&fx, rerun).await;
    assert_eq!(result_rows(&fx, run.id).await, 1, "nothing recomputed");
    mark_done(&fx, rerun).await;
    fx.tick(at + Duration::minutes(5)).await;
    assert_eq!(fx.run().await.state, "reported");
    assert_eq!(exhaustion_notices(&fx).await, 0);
}

#[tokio::test]
async fn an_aborted_runs_failed_estimate_whose_result_exists_is_re_run_to_done() {
    let (fx, at) = aborted().await;
    let run = fx.run().await;
    fx.tick(at + Duration::minutes(5)).await;
    let first = fx
        .run()
        .await
        .estimate_task_id
        .expect("aborted enqueued it");
    run_queued(&fx, first).await;
    let failed_at = at + Duration::minutes(10);
    fail(&fx, failed_at).await;

    let rerun_at = failed_at + backoff();
    fx.tick(rerun_at).await;
    let rerun = fx.run().await.estimate_task_id.expect("re-enqueued");
    assert_ne!(rerun, first);
    run_queued(&fx, rerun).await;
    assert_eq!(result_rows(&fx, run.id).await, 1);
    mark_done(&fx, rerun).await;
    let done_at = rerun_at + Duration::minutes(5);
    fx.tick(done_at).await;
    assert_eq!(fx.run().await.state, "aborted");
    assert_eq!(estimate_json(&fx, done_at).await["state"], "done");
    assert_eq!(exhaustion_notices(&fx).await, 0);
}

#[tokio::test]
async fn an_aborted_run_is_estimated_once_and_its_result_is_labelled_aborted() {
    let (fx, at) = aborted().await;
    let run = fx.run().await;
    assert!(
        run.estimate_task_id.is_none(),
        "not on the step into aborted"
    );
    assert!(
        run.next_check_at <= at + Duration::seconds(RUN_CHECK_INTERVAL_SECS),
        "an owed estimate keeps an aborted run on the short cadence: {}",
        run.next_check_at
    );

    fx.tick(at + Duration::minutes(5)).await;
    fx.tick(at + Duration::minutes(10)).await;
    assert_eq!(fx.queued_kind(ESTIMATE_KIND).await, 1);
    let task = fx
        .run()
        .await
        .estimate_task_id
        .expect("aborted enqueued it");
    run_queued(&fx, task).await;
    let row = result_of(&fx.db, run.id).await.expect("the result row");
    assert!(row.aborted);

    mark_done(&fx, task).await;
    let done_at = at + Duration::minutes(15);
    fx.tick(done_at).await;
    let run = fx.run().await;
    assert_eq!(run.state, "aborted");
    assert_eq!(estimate_json(&fx, done_at).await["state"], "done");
    assert!(run.next_check_at > done_at + Duration::hours(1), "settled");
    assert_eq!(fx.queued_kind(ESTIMATE_KIND).await, 1);
}

#[tokio::test]
async fn an_aborted_runs_failed_estimate_follows_the_same_retry_path() {
    let (fx, at) = aborted().await;
    fx.tick(at + Duration::minutes(5)).await;
    let failed_at = at + Duration::minutes(30);
    let first = fail(&fx, failed_at).await;
    let waiting = failed_at + Duration::minutes(5);
    fx.tick(waiting).await;
    let run = fx.run().await;
    assert_eq!(run.estimate_task_id, Some(first), "inside the backoff");
    assert!(
        run.next_check_at <= waiting + Duration::seconds(RUN_CHECK_INTERVAL_SECS),
        "a pending retry keeps an aborted run on the short cadence: {}",
        run.next_check_at
    );

    fx.tick(failed_at + backoff()).await;
    let second = fx.run().await.estimate_task_id.expect("re-enqueued");
    assert_ne!(second, first);
    assert_eq!(fx.queued_kind(ESTIMATE_KIND).await, 2);
    assert_eq!(fx.run().await.state, "aborted");
    assert_eq!(exhaustion_notices(&fx).await, 0);
}
