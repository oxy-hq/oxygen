//! Bounded retry queue for the span-flush bridge.
//!
//! The bridge task ([`crate::telemetry::spawn_bridge`]) used to discard a
//! batch whenever `insert_spans` failed, so any store outage silently lost
//! telemetry (incident 2026-07-06: hours of spans dropped while the Airhouse
//! session was poisoned). `SpanFlushQueue` keeps failed batches queued for
//! retry with exponential backoff, capped at `max_buffered` records so an
//! extended outage degrades to bounded, *observable* loss (oldest first)
//! instead of unbounded memory growth.
//!
//! Pure state machine — no I/O, no clock reads — so callers inject `Instant`s
//! and tests drive time explicitly.
//!
//! Generic over the record type since 2026-09: custom-app wide events and
//! function log lines are pushed through the same bridge shape as spans, and
//! they want the same bounded-loss-under-outage behaviour rather than a second,
//! subtly different implementation of it. [`SpanFlushQueue`] is the original
//! specialisation and is what the tests below exercise.

use std::time::{Duration, Instant};

use crate::types::SpanRecord;

/// The span specialisation — the shape this was written for.
pub(crate) type SpanFlushQueue = FlushQueue<SpanRecord>;

pub(crate) struct FlushQueue<T> {
    buffer: Vec<T>,
    max_buffered: usize,
    base_backoff: Duration,
    max_backoff: Duration,
    consecutive_failures: u32,
    next_attempt_at: Option<Instant>,
}

impl<T> FlushQueue<T> {
    pub(crate) fn new(max_buffered: usize, base_backoff: Duration, max_backoff: Duration) -> Self {
        Self {
            buffer: Vec::new(),
            max_buffered,
            base_backoff,
            max_backoff,
            consecutive_failures: 0,
            next_attempt_at: None,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.buffer.len()
    }

    /// Enqueue a record. Returns how many records were evicted (oldest first)
    /// to respect `max_buffered`.
    pub(crate) fn push(&mut self, record: T) -> usize {
        self.buffer.push(record);
        self.evict_overflow()
    }

    /// Take the whole buffer for sending if it holds at least `min_len`
    /// records and the failure backoff (if any) has elapsed.
    pub(crate) fn take_ready(&mut self, now: Instant, min_len: usize) -> Option<Vec<T>> {
        if self.buffer.len() < min_len.max(1) {
            return None;
        }
        if let Some(at) = self.next_attempt_at
            && now < at
        {
            return None;
        }
        Some(std::mem::take(&mut self.buffer))
    }

    /// Drain everything regardless of the backoff gate. For shutdown's final
    /// best-effort send — there is no later attempt the gate could defer to.
    pub(crate) fn take_all(&mut self) -> Vec<T> {
        std::mem::take(&mut self.buffer)
    }

    /// A send succeeded — clear the failure backoff.
    pub(crate) fn on_success(&mut self) {
        self.consecutive_failures = 0;
        self.next_attempt_at = None;
    }

    /// A send failed — requeue `batch` ahead of anything pushed since, arm
    /// the (exponential, capped) backoff. Returns how many records were
    /// evicted (oldest first) to respect `max_buffered`.
    pub(crate) fn on_failure(&mut self, mut batch: Vec<T>, now: Instant) -> usize {
        batch.append(&mut self.buffer);
        self.buffer = batch;
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        // 1 << (n-1) doublings of the base, saturating well below overflow.
        let exponent = (self.consecutive_failures - 1).min(16);
        let backoff = self
            .base_backoff
            .saturating_mul(1u32 << exponent)
            .min(self.max_backoff);
        self.next_attempt_at = Some(now + backoff);
        self.evict_overflow()
    }

    fn evict_overflow(&mut self) -> usize {
        let overflow = self.buffer.len().saturating_sub(self.max_buffered);
        if overflow > 0 {
            self.buffer.drain(..overflow);
        }
        overflow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(n: usize) -> SpanRecord {
        SpanRecord {
            trace_id: format!("trace-{n}"),
            span_id: format!("span-{n}"),
            parent_span_id: String::new(),
            span_name: "test".into(),
            service_name: "oxy".into(),
            span_attributes: "{}".into(),
            duration_ns: 0,
            status_code: "OK".into(),
            status_message: String::new(),
            event_data: "[]".into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            workspace_id: String::new(),
        }
    }

    fn queue() -> SpanFlushQueue {
        SpanFlushQueue::new(5, Duration::from_secs(1), Duration::from_secs(8))
    }

    fn ids(batch: &[SpanRecord]) -> Vec<&str> {
        batch.iter().map(|r| r.trace_id.as_str()).collect()
    }

    #[test]
    fn empty_queue_has_nothing_ready() {
        let mut q = queue();
        assert!(q.take_ready(Instant::now(), 1).is_none());
    }

    #[test]
    fn take_ready_honors_min_len() {
        let mut q = queue();
        q.push(rec(0));
        q.push(rec(1));
        assert!(
            q.take_ready(Instant::now(), 100).is_none(),
            "below min_len must not flush"
        );
        let batch = q.take_ready(Instant::now(), 1).expect("min_len met");
        assert_eq!(ids(&batch), ["trace-0", "trace-1"]);
        assert_eq!(q.len(), 0);
    }

    #[test]
    fn failed_batch_is_requeued_ahead_of_newer_records() {
        let mut q = queue();
        let now = Instant::now();
        q.push(rec(0));
        q.push(rec(1));
        let batch = q.take_ready(now, 1).unwrap();
        q.push(rec(2));
        q.on_failure(batch, now);
        assert_eq!(q.len(), 3);
        let retry = q.take_ready(now + Duration::from_secs(1), 1).unwrap();
        assert_eq!(
            ids(&retry),
            ["trace-0", "trace-1", "trace-2"],
            "requeued records must keep their original order"
        );
    }

    #[test]
    fn backoff_gates_retry_until_deadline() {
        let mut q = queue();
        let now = Instant::now();
        q.push(rec(0));
        let batch = q.take_ready(now, 1).unwrap();
        q.on_failure(batch, now);
        assert!(
            q.take_ready(now, 1).is_none(),
            "retry before backoff elapsed must be gated"
        );
        assert!(
            q.take_ready(now + Duration::from_secs(1), 1).is_some(),
            "retry after base backoff must proceed"
        );
    }

    #[test]
    fn backoff_doubles_per_consecutive_failure_and_caps() {
        let mut q = queue();
        let now = Instant::now();
        q.push(rec(0));

        // 5 consecutive failures: backoff 1s → 2s → 4s → 8s → 8s (capped).
        for _ in 0..5 {
            let batch = std::mem::take(&mut q.buffer);
            q.on_failure(batch, now);
        }
        assert!(
            q.take_ready(now + Duration::from_secs(4), 1).is_none(),
            "capped backoff must still exceed 4s"
        );
        assert!(
            q.take_ready(now + Duration::from_secs(8), 1).is_some(),
            "capped backoff must not exceed max_backoff (8s)"
        );
    }

    #[test]
    fn success_resets_backoff() {
        let mut q = queue();
        let now = Instant::now();
        q.push(rec(0));
        let batch = q.take_ready(now, 1).unwrap();
        q.on_failure(batch, now);
        let batch = q.take_ready(now + Duration::from_secs(1), 1).unwrap();
        q.on_success();
        // A later failure starts back at the base backoff, not 2× it.
        q.on_failure(batch, now + Duration::from_secs(2));
        assert!(
            q.take_ready(now + Duration::from_secs(3), 1).is_some(),
            "backoff after success must reset to base (1s)"
        );
    }

    #[test]
    fn take_all_bypasses_backoff_gate() {
        let mut q = queue();
        let now = Instant::now();
        q.push(rec(0));
        let batch = q.take_ready(now, 1).unwrap();
        q.on_failure(batch, now); // arms next_attempt_at in the future
        assert!(
            q.take_ready(now, 1).is_none(),
            "sanity: the backoff gate is armed"
        );
        let all = q.take_all();
        assert_eq!(
            ids(&all),
            ["trace-0"],
            "take_all must drain the buffer even inside the backoff window"
        );
        assert_eq!(q.len(), 0);
    }

    #[test]
    fn push_overflow_evicts_oldest() {
        let mut q = queue(); // max_buffered = 5
        let mut dropped = 0;
        for n in 0..7 {
            dropped += q.push(rec(n));
        }
        assert_eq!(dropped, 2);
        assert_eq!(q.len(), 5);
        let batch = q.take_ready(Instant::now(), 1).unwrap();
        assert_eq!(
            ids(&batch),
            ["trace-2", "trace-3", "trace-4", "trace-5", "trace-6"],
            "eviction must drop the oldest records"
        );
    }

    #[test]
    fn requeue_overflow_evicts_oldest() {
        let mut q = queue(); // max_buffered = 5
        let now = Instant::now();
        for n in 0..4 {
            q.push(rec(n));
        }
        let batch = q.take_ready(now, 1).unwrap();
        for n in 4..8 {
            q.push(rec(n));
        }
        let dropped = q.on_failure(batch, now);
        assert_eq!(dropped, 3);
        assert_eq!(q.len(), 5);
        let retry = q.take_ready(now + Duration::from_secs(1), 1).unwrap();
        assert_eq!(
            ids(&retry),
            ["trace-3", "trace-4", "trace-5", "trace-6", "trace-7"],
            "requeue eviction must drop the oldest records first"
        );
    }
}
