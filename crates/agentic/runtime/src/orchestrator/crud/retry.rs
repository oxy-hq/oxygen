//! A failed task put back on the queue for the retry its policy declares.
//!
//! The queue row carries the task's `TaskPolicy` (`policy`) and how many of
//! its retries it has spent (`retry_attempt`). A retry is the same row going
//! `claimed -> queued` again, invisible until its backoff has passed
//! (`available_at`), so the backoff is a property of the row rather than a
//! sleep in whoever drives the run, and it survives a restart. Both writes are
//! fenced on the claim that failed, and the requeue also on the attempt it
//! read, so a stale execution can neither requeue a peer's claim nor spend
//! an attempt twice.

use std::time::Duration;

use agentic_core::delegation::{RetryPolicy, TaskPolicy};
use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, FromQueryResult, Statement};

/// The retry state of a failed claim, read before deciding.
#[derive(Debug, Clone)]
pub struct RetryState {
    /// The row's declared retry policy, if it has one.
    pub retry: Option<RetryPolicy>,
    /// Retries already spent: the failed execution was attempt `spent + 1`.
    pub spent: u32,
}

impl RetryState {
    /// How many executions the task has had, the failed one included.
    pub fn attempts(&self) -> u32 {
        self.spent + 1
    }

    /// The backoff before the next attempt, or `None` when the failure is
    /// final: no policy, every retry spent, or an error `retry_on` excludes.
    pub fn next_delay(&self, error: &str) -> Option<Duration> {
        let retry = self.retry.as_ref()?;
        let matches =
            retry.retry_on.is_empty() || retry.retry_on.iter().any(|p| error.contains(p.as_str()));
        (self.spent < retry.max_retries && matches)
            .then(|| retry.backoff.delay_for_attempt(self.spent))
    }
}

/// The failure a task reports once it will not be retried: unchanged after a
/// single attempt, and naming the attempt count after more than one.
pub fn final_failure_message(attempts: u32, error: &str) -> String {
    if attempts > 1 {
        format!("failed after {attempts} attempts: {error}")
    } else {
        error.to_string()
    }
}

/// Read the retry state of `task_id` as `claim_epoch` holds it. `None` when
/// that claim no longer holds the row.
pub async fn retry_state(
    db: &impl ConnectionTrait,
    task_id: &str,
    worker_id: &str,
    claim_epoch: Option<i64>,
) -> Result<Option<RetryState>, DbErr> {
    #[derive(FromQueryResult)]
    struct Row {
        policy: Option<serde_json::Value>,
        retry_attempt: i32,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT policy, retry_attempt FROM agentic_task_queue \
         WHERE task_id = $1 AND worker_id = $2 AND queue_status = 'claimed' \
           AND ($3::bigint IS NULL OR claim_epoch = $3)",
        [task_id.into(), worker_id.into(), claim_epoch.into()],
    ))
    .one(db)
    .await?;
    Ok(row.map(|r| RetryState {
        retry: r
            .policy
            .and_then(|p| serde_json::from_value::<TaskPolicy>(p).ok())
            .and_then(|p| p.retry),
        spent: u32::try_from(r.retry_attempt).unwrap_or(0),
    }))
}

/// Put a failed claim back as `queued`, claimable after `delay`, with one
/// more retry spent. `false` when the claim, or the attempt it read, no
/// longer holds the row — nothing was written.
///
/// A fresh attempt gets a fresh crash budget: `claim_count` restarts at 0, as
/// it does on every enqueue.
pub async fn requeue_failed_attempt(
    db: &impl ConnectionTrait,
    task_id: &str,
    worker_id: &str,
    claim_epoch: Option<i64>,
    spent: u32,
    delay: Duration,
) -> Result<bool, DbErr> {
    let res = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE agentic_task_queue \
             SET queue_status = 'queued', \
                 worker_id = NULL, \
                 claimed_at = NULL, \
                 last_heartbeat = NULL, \
                 claim_count = 0, \
                 retry_attempt = retry_attempt + 1, \
                 first_deferred_at = NULL, \
                 available_at = now() + make_interval(secs => $5), \
                 updated_at = now() \
             WHERE task_id = $1 AND worker_id = $2 AND queue_status = 'claimed' \
               AND ($3::bigint IS NULL OR claim_epoch = $3) \
               AND retry_attempt = $4",
            [
                task_id.into(),
                worker_id.into(),
                claim_epoch.into(),
                (spent as i32).into(),
                delay.as_secs_f64().into(),
            ],
        ))
        .await?;
    Ok(res.rows_affected() == 1)
}

/// Cancel `task_id` if it is waiting out a retry's backoff: `queued` with a
/// retry spent. `true` when it was — no execution exists to report the Stop,
/// so the caller must.
pub async fn cancel_waiting_retry(db: &impl ConnectionTrait, task_id: &str) -> Result<bool, DbErr> {
    let res = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE agentic_task_queue SET queue_status = 'cancelled', updated_at = now() \
             WHERE task_id = $1 AND queue_status = 'queued' AND retry_attempt > 0",
            [task_id.into()],
        ))
        .await?;
    Ok(res.rows_affected() == 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::delegation::BackoffStrategy;

    fn state(max_retries: u32, spent: u32, retry_on: &[&str]) -> RetryState {
        RetryState {
            retry: Some(RetryPolicy {
                max_retries,
                backoff: BackoffStrategy::Exponential {
                    initial_delay_ms: 1_000,
                    max_delay_ms: 3_000,
                },
                retry_on: retry_on.iter().map(|s| s.to_string()).collect(),
            }),
            spent,
        }
    }

    #[test]
    fn retries_until_spent_with_doubling_capped_backoff() {
        assert_eq!(
            state(3, 0, &[]).next_delay("x"),
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            state(3, 1, &[]).next_delay("x"),
            Some(Duration::from_secs(2))
        );
        assert_eq!(
            state(3, 2, &[]).next_delay("x"),
            Some(Duration::from_secs(3))
        );
        assert_eq!(state(3, 3, &[]).next_delay("x"), None);
    }

    #[test]
    fn no_policy_or_an_excluded_error_is_final() {
        let none = RetryState {
            retry: None,
            spent: 0,
        };
        assert_eq!(none.next_delay("x"), None);
        assert_eq!(state(3, 0, &["timeout"]).next_delay("refused"), None);
        assert!(
            state(3, 0, &["timeout"])
                .next_delay("read timeout")
                .is_some()
        );
    }

    #[test]
    fn the_final_message_names_the_count_only_after_a_retry() {
        assert_eq!(final_failure_message(1, "boom"), "boom");
        assert_eq!(
            final_failure_message(3, "boom"),
            "failed after 3 attempts: boom"
        );
    }
}
