//! Background retry for opening the observability store, used by
//! [`super::finalize`] when ClickHouse is unavailable at boot.

use std::future::Future;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::MissedTickBehavior;

/// How long the background retry waits between attempts, and how many spans it
/// holds while the store is unavailable.
#[derive(Debug, Clone, Copy)]
pub(super) struct RetryPolicy {
    /// Delay before the first retry. Doubles after every failed attempt.
    pub(super) base_delay: Duration,
    /// Ceiling for the doubling — a long outage settles at one attempt per
    /// `max_delay`, which also bounds the warning log to one line per attempt.
    pub(super) max_delay: Duration,
    /// Spans kept in the channel while the store is unavailable. Oldest are
    /// dropped beyond this. Matches the bridge's own `MAX_BUFFERED_SPANS`, so
    /// an outage before the bridge starts is bounded the same way as one after.
    pub(super) max_pending: usize,
    /// How often the channel is trimmed to `max_pending` — during the backoff
    /// wait AND while an attempt is in flight, so a slow or hung attempt cannot
    /// lift the bound. The bound is `max_pending` plus one interval of spans.
    pub(super) trim_interval: Duration,
}

pub(super) const RETRY_POLICY: RetryPolicy = RetryPolicy {
    base_delay: Duration::from_secs(1),
    max_delay: Duration::from_secs(60),
    max_pending: 5_000,
    trim_interval: Duration::from_secs(1),
};

/// Why the ClickHouse store could not be opened. Decides whether
/// [`super::finalize`] retries.
#[derive(Debug)]
pub(super) enum OpenError {
    /// The client could not be built from `OXY_CLICKHOUSE_*`. Retrying the same
    /// environment cannot fix it.
    Config(String),
    /// The server did not answer the probe or did not accept the schema DDL —
    /// unreachable, restarting, or refusing connections. Transient by assumption.
    Unavailable(String),
}

impl OpenError {
    pub(super) fn message(&self) -> &str {
        match self {
            OpenError::Config(msg) | OpenError::Unavailable(msg) => msg,
        }
    }
}

/// Drop the oldest records until at most `max_pending` remain queued. Returns
/// how many were dropped.
fn trim_backlog<R>(receiver: &mut UnboundedReceiver<R>, max_pending: usize) -> usize {
    let mut dropped = 0;
    while receiver.len() > max_pending {
        if receiver.try_recv().is_err() {
            break;
        }
        dropped += 1;
    }
    dropped
}

/// Drive `fut` to completion, trimming `receiver` every `policy.trim_interval`
/// until it finishes. Adds the number of dropped records to `dropped`.
async fn while_trimming<R, T>(
    receiver: &mut UnboundedReceiver<R>,
    fut: impl Future<Output = T>,
    policy: RetryPolicy,
    dropped: &mut usize,
) -> T {
    tokio::pin!(fut);
    let mut tick = tokio::time::interval(policy.trim_interval);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            out = &mut fut => return out,
            _ = tick.tick() => *dropped += trim_backlog(receiver, policy.max_pending),
        }
    }
}

/// Call `open` until it yields a store, waiting `policy.base_delay` before the
/// first call and doubling up to `policy.max_delay` after each
/// [`OpenError::Unavailable`]. Returns `None` on [`OpenError::Config`], which
/// retrying cannot fix.
///
/// `receiver` is trimmed to `policy.max_pending` throughout — while waiting and
/// while `open` runs — so an outage costs bounded memory rather than an
/// unbounded channel's worth of spans. `open` is not timed here: bounding the
/// part of it that can hang is the caller's job (see `PROBE_TIMEOUT`), because
/// the rest can legitimately run long.
pub(super) async fn retry_until_open<T, R, F, Fut>(
    receiver: &mut UnboundedReceiver<R>,
    mut open: F,
    policy: RetryPolicy,
) -> Option<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, OpenError>>,
{
    let mut delay = policy.base_delay;
    let mut attempts: u32 = 0;
    let mut dropped: usize = 0;

    loop {
        while_trimming(receiver, tokio::time::sleep(delay), policy, &mut dropped).await;

        attempts += 1;
        match while_trimming(receiver, open(), policy, &mut dropped).await {
            Ok(store) => {
                tracing::info!(
                    attempts,
                    dropped_spans = dropped,
                    "Observability store reachable; span capture started"
                );
                return Some(store);
            }
            Err(OpenError::Unavailable(msg)) => {
                delay = (delay * 2).min(policy.max_delay);
                tracing::warn!(
                    attempts,
                    dropped_spans = dropped,
                    next_retry_secs = delay.as_secs(),
                    error = %msg,
                    "Observability store still unavailable; spans are not being recorded"
                );
            }
            Err(OpenError::Config(msg)) => {
                tracing::error!(attempts, error = %msg, "Observability store misconfigured; giving up");
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    const TEST_POLICY: RetryPolicy = RetryPolicy {
        base_delay: Duration::from_secs(1),
        max_delay: Duration::from_secs(4),
        max_pending: 3,
        trim_interval: Duration::from_millis(250),
    };

    #[test]
    fn trim_backlog_drops_oldest_beyond_cap() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        for i in 0..10u32 {
            tx.send(i).unwrap();
        }

        assert_eq!(trim_backlog(&mut rx, 3), 7);
        assert_eq!(rx.try_recv().unwrap(), 7);
        assert_eq!(rx.try_recv().unwrap(), 8);
        assert_eq!(rx.try_recv().unwrap(), 9);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn trim_backlog_leaves_short_queue_alone() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tx.send(1u32).unwrap();

        assert_eq!(trim_backlog(&mut rx, 3), 0);
        assert_eq!(rx.len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn retries_with_doubling_backoff_until_the_store_opens() {
        let (_tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<u32>();
        let calls = AtomicU32::new(0);
        let started = tokio::time::Instant::now();

        let opened = retry_until_open(
            &mut rx,
            || {
                let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
                async move {
                    if n < 3 {
                        Err(OpenError::Unavailable("connection refused".into()))
                    } else {
                        Ok("store")
                    }
                }
            },
            TEST_POLICY,
        )
        .await;

        assert_eq!(opened, Some("store"));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        // Waits 1s, 2s, then 4s before the third (successful) call.
        assert_eq!(started.elapsed(), Duration::from_secs(7));
    }

    #[tokio::test(start_paused = true)]
    async fn backoff_is_capped_at_max_delay() {
        let (_tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<u32>();
        let calls = AtomicU32::new(0);
        let started = tokio::time::Instant::now();

        let opened = retry_until_open(
            &mut rx,
            || {
                let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
                async move {
                    if n < 5 {
                        Err(OpenError::Unavailable("connection refused".into()))
                    } else {
                        Ok(())
                    }
                }
            },
            TEST_POLICY,
        )
        .await;

        assert_eq!(opened, Some(()));
        // 1 + 2 + 4 + 4 + 4: the doubling stops at max_delay (4s).
        assert_eq!(started.elapsed(), Duration::from_secs(15));
    }

    #[tokio::test(start_paused = true)]
    async fn config_error_stops_retrying() {
        let (_tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<u32>();
        let calls = AtomicU32::new(0);

        let opened: Option<()> = retry_until_open(
            &mut rx,
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Err(OpenError::Config("bad url".into())) }
            },
            TEST_POLICY,
        )
        .await;

        assert_eq!(opened, None);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn backlog_stays_bounded_while_waiting() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<u32>();
        for i in 0..100 {
            tx.send(i).unwrap();
        }

        let opened = retry_until_open(&mut rx, || async { Ok(()) }, TEST_POLICY).await;

        assert_eq!(opened, Some(()));
        // Trimmed during the first wait; the newest records survive.
        assert_eq!(rx.len(), TEST_POLICY.max_pending);
        assert_eq!(rx.try_recv().unwrap(), 97);
    }

    /// Review finding on #3183: trimming used to run only during the backoff
    /// sleep, so an attempt that parked (a ClickHouse that accepts the
    /// connection and never answers) let the channel grow without bound.
    #[tokio::test(start_paused = true)]
    async fn backlog_stays_bounded_while_an_attempt_is_in_flight() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<u32>();

        let opened = retry_until_open(
            &mut rx,
            move || {
                let tx = tx.clone();
                async move {
                    // Spans keep arriving while the attempt is parked.
                    for i in 0..100 {
                        tx.send(i).unwrap();
                    }
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    Ok(())
                }
            },
            TEST_POLICY,
        )
        .await;

        assert_eq!(opened, Some(()));
        assert_eq!(rx.len(), TEST_POLICY.max_pending);
        assert_eq!(rx.try_recv().unwrap(), 97);
    }
}
