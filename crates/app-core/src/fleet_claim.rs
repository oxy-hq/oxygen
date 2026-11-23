//! Fleet-wide claims for process-lifetime jobs: one `fleet_job_claims` row per
//! job, so a job every replica of a leaderless fleet starts (the `oxy worker`s)
//! still does its work once per interval across all of them.
//!
//! The same shape as the periodic driver's workspace claim
//! (`oxy-server`'s `drivers/periodic/claim.rs`): every statement uses the
//! database's clock, never this process's, and the claim is one statement, so
//! there is no leader to elect and no lease to renew. The upsert's `WHERE`
//! runs against the row as the winner left it — Postgres locks the conflicting
//! row and re-checks — so of two replicas claiming in the same instant exactly
//! one gets a row back.
//!
//! Two claims:
//!
//! - [`claim_interval`]: at most once per `interval` (the storage sweeper,
//!   the usage-backfill sweep, the audit anchor). [`run_claimed`] polls it.
//! - [`claim_once`]: once per `key` (the asset-bucket lifecycle check, once per
//!   build however many replicas boot it).
//!
//! A replica that dies mid-pass loses that interval's pass; the next interval
//! is claimable as usual. Every caller is a re-measure or a sweep whose next
//! pass covers the missed one, which is what makes that acceptable.

use std::future::Future;
use std::time::Duration;

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};

/// The shortest poll [`run_claimed`] uses, however short the interval.
const MIN_POLL: Duration = Duration::from_secs(1);
/// The longest: a claim falls due at most this late.
const MAX_POLL: Duration = Duration::from_secs(60);

fn statement(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// Which process took a claim, for whoever reads the table during an incident.
/// Display only: nothing compares it.
fn claimant() -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| format!("pid-{}", std::process::id()))
}

/// Claim `job` for the next `interval` if it is due: `true` means this process
/// runs the job now and nobody else does until `interval` has passed.
///
/// A job with no row yet is due, so the first replica to ask after a fresh
/// migration runs it.
pub async fn claim_interval(
    db: &DatabaseConnection,
    job: &str,
    interval: Duration,
) -> Result<bool, DbErr> {
    let millis = i64::try_from(interval.as_millis()).unwrap_or(i64::MAX);
    let row = db
        .query_one_raw(statement(
            "INSERT INTO fleet_job_claims (job, next_run_at, claimed_at, claimed_by) \
             VALUES ($1, now() + $2::bigint * interval '1 millisecond', now(), $3) \
             ON CONFLICT (job) DO UPDATE \
               SET next_run_at = EXCLUDED.next_run_at, \
                   claimed_at = EXCLUDED.claimed_at, \
                   claimed_by = EXCLUDED.claimed_by \
             WHERE fleet_job_claims.next_run_at <= now() \
             RETURNING job",
            vec![job.into(), millis.into(), claimant().into()],
        ))
        .await?;
    Ok(row.is_some())
}

/// Claim `job` for `key` if it has not run for that key yet: `true` once per
/// distinct key across the fleet.
///
/// Only the last key is kept, so a key that comes back after another (an old
/// build's replica restarting mid-rollout) runs again. That is one extra run of
/// a read-only check, not a correctness problem.
pub async fn claim_once(db: &DatabaseConnection, job: &str, key: &str) -> Result<bool, DbErr> {
    let row = db
        .query_one_raw(statement(
            "INSERT INTO fleet_job_claims (job, next_run_at, claimed_at, claimed_by, claim_key) \
             VALUES ($1, now(), now(), $3, $2) \
             ON CONFLICT (job) DO UPDATE \
               SET claim_key = EXCLUDED.claim_key, \
                   claimed_at = EXCLUDED.claimed_at, \
                   claimed_by = EXCLUDED.claimed_by \
             WHERE fleet_job_claims.claim_key IS DISTINCT FROM EXCLUDED.claim_key \
             RETURNING job",
            vec![job.into(), key.into(), claimant().into()],
        ))
        .await?;
    Ok(row.is_some())
}

/// How often [`run_claimed`] asks: a tenth of the interval, within
/// [`MIN_POLL`]..=[`MAX_POLL`]. A pass therefore starts at most one poll after
/// it falls due, and a failed claim costs one cheap statement per poll.
pub fn poll_period(interval: Duration) -> Duration {
    (interval / 10).clamp(MIN_POLL, MAX_POLL)
}

/// Run `pass` whenever this process wins [`claim_interval`] for `job`, until
/// `shutdown` resolves. Start it on every replica: the claim makes it once per
/// interval across them.
///
/// A pass in flight is dropped at shutdown; the next interval is claimable as
/// usual. A failed claim is logged and treated as lost.
pub async fn run_claimed<S, F, Fut>(
    db: DatabaseConnection,
    job: &'static str,
    interval: Duration,
    shutdown: S,
    mut pass: F,
) where
    S: Future<Output = ()>,
    F: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    let mut poll = tokio::time::interval(poll_period(interval));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => return,
            _ = poll.tick() => {}
        }
        match claim_interval(&db, job, interval).await {
            Ok(true) => {
                tokio::select! {
                    _ = &mut shutdown => return,
                    _ = pass() => {}
                }
            }
            Ok(false) => {}
            Err(e) => tracing::warn!(job, error = %e, "fleet job claim failed; skipping this poll"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_is_a_tenth_of_the_interval_within_bounds() {
        assert_eq!(
            poll_period(Duration::from_secs(300)),
            Duration::from_secs(30)
        );
        assert_eq!(poll_period(Duration::from_secs(3600)), MAX_POLL);
        assert_eq!(poll_period(Duration::from_secs(2)), MIN_POLL);
        assert_eq!(poll_period(Duration::ZERO), MIN_POLL);
    }
}
