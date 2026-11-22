//! Telling airhouse's "not now" apart from its "no", and how long to wait.
//!
//! # Why this exists, and why it must not be "simplified" away
//!
//! Airhouse's shared data plane admits a bounded number of concurrent pgwire
//! connections per pod across all tenants, queues a few more, and gives up on a
//! queued connect after a timeout. A permit is held for a connection's entire
//! lifetime, not per query. When the cap is reached it answers SQLSTATE
//! **`53300`**.
//!
//! `53300` is backpressure, not a failure: it says *this moment* has no room,
//! and the very next moment probably does. Surfacing it to the user as a query
//! error — which is what a plain `?` on the build does — turns a wait of
//! milliseconds into a failed Dev Portal query. When that gate fills, what is
//! scarce is connection *setup*, not CPU or memory, so retrying a moment later
//! is exactly the right move and scaling out is not.
//!
//! **Retrying is safe here because this classifier is only ever applied to a
//! connect**, never to a statement. A connect that never completed had no
//! effect to repeat. Do not reuse [`is_backpressure`] to retry a query — a
//! `53300` arriving mid-session is a different question, and an `INSERT` that
//! may or may not have landed must not be replayed.

use std::time::Duration;

/// Waits between connect attempts. Short, because the thing being waited on is
/// another request finishing — airhouse's permits turn over in milliseconds
/// once anything completes, so a long local backoff just spends the caller's
/// latency budget to arrive later.
///
/// Four attempts over ~1.4s total. Past that the data plane is not transiently
/// busy but genuinely full, and the caller deserves the error rather than a
/// request that hangs.
const BACKOFFS: &[Duration] = &[
    Duration::from_millis(100),
    Duration::from_millis(300),
    Duration::from_millis(1000),
];

/// How long to wait before attempt `attempt` (0-based), or `None` when the
/// attempts are used up.
pub(super) fn retry_delay(attempt: usize) -> Option<Duration> {
    BACKOFFS.get(attempt).copied()
}

/// The number of connect attempts [`retry_delay`] allows in total.
pub(super) fn max_attempts() -> usize {
    BACKOFFS.len() + 1
}

/// True when a failed *connect* says "not now" rather than "not ever".
///
/// Matches the SQLSTATE in both shapes this path can produce it: the airhouse
/// connector's `format_pg_error` writes `[SQLSTATE 53300]`, while the error
/// text cameras logs is framed `[53300]`. Matching the framed code rather than
/// a bare `53300` keeps a tenant id or a byte count from reading as a SQLSTATE.
pub(super) fn is_backpressure(err: &str) -> bool {
    err.contains("[SQLSTATE 53300]") || err.contains("[53300]") || at_capacity_phrase(err)
}

/// Airhouse's own wording for the same condition, for the paths that report the
/// message without a SQLSTATE frame.
fn at_capacity_phrase(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("too many connections") || lower.contains("too many clients")
}

#[cfg(test)]
mod tests {
    use super::{BACKOFFS, is_backpressure, max_attempts, retry_delay};

    #[test]
    fn the_sqlstate_is_recognised_in_both_framings() {
        // What `format_pg_error` in crates/airhouse/src/connector produces.
        assert!(is_backpressure(
            "error connecting to server [SQLSTATE 53300] too many connections for tenant"
        ));
        // What the cameras path's `pg_error_text` produces.
        assert!(is_backpressure("[53300] too many connections for tenant"));
    }

    #[test]
    fn the_wording_alone_is_enough() {
        assert!(is_backpressure("FATAL: too many clients already"));
        assert!(is_backpressure("Too Many Connections For Tenant"));
    }

    #[test]
    fn a_real_failure_is_not_retried() {
        // Retrying any of these just makes the user wait longer for the same
        // answer, and the first one would retry a permission error.
        assert!(!is_backpressure(
            "[SQLSTATE 28000] password authentication failed for user \"eph_abc\""
        ));
        assert!(!is_backpressure(
            "[SQLSTATE 3D000] database \"ws_nope\" does not exist"
        ));
        assert!(!is_backpressure(
            "[XX000] Catalog Error: Table with name orders does not exist!"
        ));
        assert!(!is_backpressure(
            "airhouse_managed: AIRHOUSE_WIRE_HOST is not configured"
        ));
    }

    #[test]
    fn a_bare_number_is_not_a_sqlstate() {
        // A tenant id or row count that happens to contain the digits must not
        // turn a hard failure into a retry loop.
        assert!(!is_backpressure("[SQLSTATE 42601] syntax error near 53300"));
        assert!(!is_backpressure("copied 53300 rows"));
    }

    #[test]
    fn the_attempt_budget_runs_out() {
        assert_eq!(max_attempts(), BACKOFFS.len() + 1);
        for i in 0..BACKOFFS.len() {
            assert!(retry_delay(i).is_some(), "attempt {i} should have a delay");
        }
        assert_eq!(retry_delay(BACKOFFS.len()), None);
        assert_eq!(retry_delay(99), None);
    }

    #[test]
    fn the_backoff_stays_a_small_share_of_a_request_budget() {
        // Our own retries must cost only a fraction of the time airhouse will
        // itself spend queueing the same connect, or we add latency to a
        // request that was going to be served anyway. That upstream timeout is
        // configured with the data plane, not here, so this pins the local
        // budget at a couple of seconds — well inside any plausible value.
        let total: std::time::Duration = BACKOFFS.iter().sum();
        assert!(
            total < std::time::Duration::from_secs(5),
            "local backoff {total:?} should stay a small share of a request's budget"
        );
    }

    #[test]
    fn the_backoff_grows() {
        for pair in BACKOFFS.windows(2) {
            assert!(pair[1] > pair[0], "backoff should increase: {pair:?}");
        }
    }
}
