//! Cross-process wakes for a run's event stream.
//!
//! [`RuntimeState::notifiers`](super::state::RuntimeState) is rung by the
//! driver in *this* process. A run driven on another pod appends to
//! `agentic_run_events` in Postgres and rings a notifier this process cannot
//! see, so a stream served here used to learn of the rows only by polling.
//! Now every append is paired with `NOTIFY oxy_run_events, '<run_id>'`
//! ([`RUN_EVENTS_CHANNEL`], issued by `crud::events` in the insert's own
//! transaction), the task router's existing LISTEN connection hears it, and
//! [`RunEventWaker::wake_run`] rings the [`RemoteWakes`] entry the stream
//! subscribed. The stream's timer stays as a safety net for a listener outage.
//!
//! A registry of its own rather than `notifiers`, on purpose. Every pod hears
//! every NOTIFY, its own appends included, so ringing `notifiers` from the
//! listener would wake a locally-driven stream twice per flush — once from the
//! driver, once from the echo — and the second wake is a `get_events_after`
//! that returns nothing. That doubles the read load on the streams that are
//! most of the open ones (`internal-docs/worker-fleet.md`, *Polling and the
//! connection pool*: count every query in the loop). Only a stream whose
//! driver may be elsewhere subscribes here; every other stream is untouched.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;
use tokio::sync::Notify;

/// The Postgres NOTIFY channel an append to `agentic_run_events` rings, with
/// the run id as its payload. Owned by the lifecycle layer because the write
/// path is here; the router re-exports it next to its task channel.
pub const RUN_EVENTS_CHANNEL: &str = "oxy_run_events";

/// What a LISTEN connection calls when a run id arrives on
/// [`RUN_EVENTS_CHANNEL`]. Implemented by [`RemoteWakes`]; a trait so the
/// listener, which is built before the runtime state exists, can be bound to
/// it late and tested against a stand-in.
pub trait RunEventWaker: Send + Sync {
    /// Ring the subscribers of `run_id`. `false` when none are parked here.
    fn wake_run(&self, run_id: &str) -> bool;
    /// Ring every subscriber: the listener reconnected and may have missed
    /// notifications, so each stream reads once to catch up.
    fn wake_all_runs(&self);
}

/// Per-run wakes for streams whose driver may be in another process.
#[derive(Default)]
pub struct RemoteWakes {
    slots: DashMap<String, Slot>,
    next_seat: AtomicU64,
}

/// The streams of one run. **One `Notify` per stream**, not one per run:
/// `notify_one` wakes a single waiter, so two tabs on the same run sharing a
/// `Notify` would take turns — one woke on the NOTIFY, the other waited for
/// its timer and saw the terminal event up to a safety-net interval late.
#[derive(Default)]
struct Slot {
    seats: Vec<(u64, Arc<Notify>)>,
}

impl RemoteWakes {
    pub fn new() -> Self {
        Self::default()
    }

    /// Park on NOTIFYs for `run_id` until the subscription drops. Each
    /// subscription has a `Notify` of its own; the run's entry leaves with
    /// its last subscriber.
    pub fn subscribe(self: &Arc<Self>, run_id: &str) -> RemoteWakeSubscription {
        let seat = self.next_seat.fetch_add(1, Ordering::Relaxed);
        let notify = Arc::new(Notify::new());
        self.slots
            .entry(run_id.to_string())
            .or_default()
            .seats
            .push((seat, Arc::clone(&notify)));
        RemoteWakeSubscription {
            registry: Arc::clone(self),
            run_id: run_id.to_string(),
            seat,
            notify,
        }
    }

    /// Is any stream parked here for `run_id`?
    pub fn is_subscribed(&self, run_id: &str) -> bool {
        self.slots.contains_key(run_id)
    }

    /// How many runs have a subscriber in this process.
    pub fn subscribed_runs(&self) -> usize {
        self.slots.len()
    }
}

impl RunEventWaker for RemoteWakes {
    /// `notify_one` on every seat, as [`RuntimeState::notify`](super::state::RuntimeState::notify)
    /// does for its single notifier: the permit is stored when that stream is
    /// not parked, so one that is mid-read when the NOTIFY lands still wakes
    /// once it parks.
    fn wake_run(&self, run_id: &str) -> bool {
        match self.slots.get(run_id) {
            Some(slot) => {
                for (_, notify) in &slot.seats {
                    notify.notify_one();
                }
                true
            }
            None => false,
        }
    }

    fn wake_all_runs(&self) {
        for slot in self.slots.iter() {
            for (_, notify) in &slot.seats {
                notify.notify_one();
            }
        }
    }
}

/// One stream's seat in [`RemoteWakes`]. Dropping it — the stream ended, or
/// its client went away — releases the seat, so the registry never outgrows
/// the streams that are actually open.
pub struct RemoteWakeSubscription {
    registry: Arc<RemoteWakes>,
    run_id: String,
    seat: u64,
    notify: Arc<Notify>,
}

impl RemoteWakeSubscription {
    /// Resolve at the next NOTIFY for this run (or at once, on a stored permit).
    pub async fn notified(&self) {
        self.notify.notified().await;
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }
}

impl Drop for RemoteWakeSubscription {
    fn drop(&mut self) {
        // The guard from `get_mut` is released at the end of the `let`, before
        // `remove_if` takes the same shard.
        let now_empty = match self.registry.slots.get_mut(&self.run_id) {
            Some(mut slot) => {
                slot.seats.retain(|(seat, _)| *seat != self.seat);
                slot.seats.is_empty()
            }
            None => false,
        };
        if now_empty {
            self.registry
                .slots
                .remove_if(&self.run_id, |_, slot| slot.seats.is_empty());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    async fn rang(sub: &RemoteWakeSubscription) -> bool {
        tokio::time::timeout(Duration::from_millis(50), sub.notified())
            .await
            .is_ok()
    }

    #[tokio::test]
    async fn a_wake_reaches_only_the_run_it_names() {
        let wakes = Arc::new(RemoteWakes::new());
        let a = wakes.subscribe("run-a");
        let b = wakes.subscribe("run-b");
        assert!(wakes.wake_run("run-a"));
        assert!(rang(&a).await, "the named run's stream must wake");
        assert!(!rang(&b).await, "another run's stream must stay parked");
    }

    #[tokio::test]
    async fn an_unknown_run_is_ignored() {
        let wakes = Arc::new(RemoteWakes::new());
        let a = wakes.subscribe("run-a");
        assert!(!wakes.wake_run("nobody-streams-this"));
        assert!(!rang(&a).await);
    }

    #[tokio::test]
    async fn a_wake_that_lands_mid_read_is_kept_as_a_permit() {
        let wakes = Arc::new(RemoteWakes::new());
        let a = wakes.subscribe("run-a");
        // The stream is reading rows, not parked, when the NOTIFY arrives.
        wakes.wake_run("run-a");
        assert!(
            rang(&a).await,
            "the permit must be consumed by the next park"
        );
        assert!(!rang(&a).await, "and only once");
    }

    /// Two tabs on one run: both parked at once, both woken by one NOTIFY.
    #[tokio::test]
    async fn every_stream_of_a_run_wakes_on_one_notify() {
        let wakes = Arc::new(RemoteWakes::new());
        let first = wakes.subscribe("run-a");
        let second = wakes.subscribe("run-a");
        let parked = tokio::join!(
            async { tokio::time::timeout(Duration::from_millis(500), first.notified()).await },
            async { tokio::time::timeout(Duration::from_millis(500), second.notified()).await },
            async {
                tokio::time::sleep(Duration::from_millis(20)).await;
                assert!(wakes.wake_run("run-a"));
            },
        );
        assert!(parked.0.is_ok(), "the first stream must wake");
        assert!(parked.1.is_ok(), "the second stream must wake too");
    }

    #[tokio::test]
    async fn the_entry_leaves_with_its_last_subscriber() {
        let wakes = Arc::new(RemoteWakes::new());
        let first = wakes.subscribe("run-a");
        let second = wakes.subscribe("run-a");
        assert_eq!(wakes.subscribed_runs(), 1);
        drop(first);
        assert!(wakes.is_subscribed("run-a"), "one stream is still open");
        assert!(wakes.wake_run("run-a"));
        assert!(rang(&second).await);
        drop(second);
        assert!(!wakes.is_subscribed("run-a"));
        assert_eq!(wakes.subscribed_runs(), 0);
        assert!(!wakes.wake_run("run-a"));
    }

    #[tokio::test]
    async fn a_reconnect_catch_up_reaches_every_subscribed_run() {
        let wakes = Arc::new(RemoteWakes::new());
        let a = wakes.subscribe("run-a");
        let b = wakes.subscribe("run-b");
        wakes.wake_all_runs();
        assert!(rang(&a).await);
        assert!(rang(&b).await);
    }
}
