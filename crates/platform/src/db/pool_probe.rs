//! What a failed pool health probe means, decided without a database.
//!
//! The monitor loop in `client.rs` owns the I/O; this owns the two judgements
//! that have to agree with each other — the `oxy_reason` label on
//! `oxy_db_pool_probe_failures_total`, and the `cause` on the "pool is starved"
//! log line — so both can be tested on plain values.

use oxy_telemetry::metrics::record::{
    DB_POOL_PROBE_FAILURE_ERROR, DB_POOL_PROBE_FAILURE_SERVER_UNAVAILABLE,
    DB_POOL_PROBE_FAILURE_TIMEOUT,
};
use tokio::time::error::Elapsed;

/// Label one probe, or `None` if it acquired.
///
/// `pool_size` must be read **after** the timed-out `acquire()` was dropped.
/// sqlx counts a connection it is still opening toward `size`, and dropping the
/// probe's own attempt releases that slot, so the value read then is what the
/// pool held without the probe.
///
/// A timeout is split by that size, because sqlx cannot tell us which side of
/// the wire we waited on. It retries a refused connect, `53300` (too many
/// clients) and `57P03` (starting up) inside `acquire()` until its own 30 s
/// deadline, so the probe's 2 s always expires first and a server outage used
/// to be recorded as `timeout` — the label that says "the pool is full".
/// Proven on dev on 2026-09-24: a Postgres restart produced three `timeout`s
/// while the pools held 2–4 of 20 connections.
///
/// - Below the ceiling, the pool was free to open a connection and did not get
///   one: the wait was on the server → `server_unavailable`.
/// - At the ceiling, nothing could be opened and nothing was returned → the
///   pool is exhausted → `timeout`.
///
/// The one case this reads wrong: during a server outage under load, other
/// callers' in-flight connects also count toward `size`, so enough of them can
/// fill the pool with no live connection at all and the label says `timeout`.
/// The log line's `cause` opens its own connection outside the pool and is the
/// arbiter there.
pub(super) fn failure_reason<T, E>(
    probe: &Result<Result<T, E>, Elapsed>,
    pool_size: u32,
    pool_max: u32,
) -> Option<&'static str> {
    match probe {
        Ok(Ok(_)) => None,
        Ok(Err(_)) => Some(DB_POOL_PROBE_FAILURE_ERROR),
        Err(_) if pool_size < pool_max => Some(DB_POOL_PROBE_FAILURE_SERVER_UNAVAILABLE),
        Err(_) => Some(DB_POOL_PROBE_FAILURE_TIMEOUT),
    }
}

/// The log line's `cause` when a connection opened outside the pool succeeds.
///
/// That success alone used to be read as "the limit is local", which is true
/// only when the pool was exhausted. A `server_unavailable` probe had room and
/// still waited, so a server that answers *now* has recovered from something —
/// saying the pool is at its ceiling there would contradict the label beside it.
pub(super) fn cause_when_server_accepts(reason: &str) -> &'static str {
    match reason {
        DB_POOL_PROBE_FAILURE_TIMEOUT => {
            "the server still accepts new connections, so the limit is local: the pool is at \
             its own ceiling, or something is holding connections without releasing them"
        }
        DB_POOL_PROBE_FAILURE_SERVER_UNAVAILABLE => {
            "the pool had room but could not open a connection in time, so the wait was on the \
             server; it accepts new connections again now, so it was briefly unavailable (a \
             restart or failover) or slow to accept — not a pool that is too small"
        }
        _ => {
            "the pool's checkout failed outright, but the server accepts a new connection now, \
             so that failure did not persist"
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use oxy_telemetry::metrics::record::DB_POOL_PROBE_FAILURE_REASONS;

    use super::*;

    const MAX: u32 = 20;

    /// A real `Elapsed`: its constructor is private, so time one out.
    async fn elapsed() -> Elapsed {
        tokio::time::timeout(Duration::ZERO, std::future::pending::<()>())
            .await
            .unwrap_err()
    }

    fn timed_out(e: Elapsed) -> Result<Result<(), ()>, Elapsed> {
        Err(e)
    }

    #[tokio::test]
    async fn a_timeout_at_the_ceiling_is_an_exhausted_pool() {
        assert_eq!(
            failure_reason(&timed_out(elapsed().await), MAX, MAX),
            Some(DB_POOL_PROBE_FAILURE_TIMEOUT)
        );
    }

    /// The dev restart of 2026-09-24: 2–4 of 20 held, and every probe timed out.
    #[tokio::test]
    async fn a_timeout_with_room_in_the_pool_waited_on_the_server() {
        for size in [0, 3, MAX - 1] {
            assert_eq!(
                failure_reason(&timed_out(elapsed().await), size, MAX),
                Some(DB_POOL_PROBE_FAILURE_SERVER_UNAVAILABLE),
                "size {size} of {MAX} left room to open a connection"
            );
        }
    }

    /// An error is an error whatever the pool held — the size only splits a timeout.
    #[test]
    fn an_acquire_error_is_error_at_any_size() {
        for size in [0, MAX] {
            assert_eq!(
                failure_reason::<(), ()>(&Ok(Err(())), size, MAX),
                Some(DB_POOL_PROBE_FAILURE_ERROR)
            );
        }
    }

    #[test]
    fn an_acquired_probe_is_not_a_failure() {
        assert_eq!(failure_reason::<(), ()>(&Ok(Ok(())), MAX, MAX), None);
    }

    /// Every label the classifier can produce is seeded at zero on install, so
    /// the first real failure is a visible delta rather than a series' birth.
    #[tokio::test]
    async fn every_reason_the_classifier_returns_is_seeded() {
        let outcomes = [
            failure_reason(&timed_out(elapsed().await), MAX, MAX),
            failure_reason(&timed_out(elapsed().await), 0, MAX),
            failure_reason::<(), ()>(&Ok(Err(())), MAX, MAX),
        ];
        for reason in outcomes.into_iter().flatten() {
            assert!(
                DB_POOL_PROBE_FAILURE_REASONS.contains(&reason),
                "{reason} is not in DB_POOL_PROBE_FAILURE_REASONS, so it would be born \
                 un-seeded: {DB_POOL_PROBE_FAILURE_REASONS:?}"
            );
        }
    }

    #[test]
    fn only_an_exhausted_pool_is_blamed_on_the_pool() {
        let local = "the limit is local";
        assert!(cause_when_server_accepts(DB_POOL_PROBE_FAILURE_TIMEOUT).contains(local));
        for reason in [
            DB_POOL_PROBE_FAILURE_SERVER_UNAVAILABLE,
            DB_POOL_PROBE_FAILURE_ERROR,
        ] {
            assert!(
                !cause_when_server_accepts(reason).contains(local),
                "{reason} must not blame the pool's ceiling"
            );
        }
    }

    #[test]
    fn a_server_side_wait_that_cleared_says_the_server_recovered() {
        let cause = cause_when_server_accepts(DB_POOL_PROBE_FAILURE_SERVER_UNAVAILABLE);
        assert!(cause.contains("the wait was on the server"), "{cause}");
    }
}
