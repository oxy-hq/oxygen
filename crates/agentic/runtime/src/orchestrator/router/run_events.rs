//! Fan-out of `oxy_run_events` notifications into this process's stream
//! registry.
//!
//! The listener is started before the `RuntimeState` whose streams it wakes
//! exists (`oxy-app` builds the router first and the state around it), so the
//! sink is bound late through [`RunEventWaker`]. Until it is bound, or when no
//! stream here is parked on the run named, a notification is dropped on the
//! floor: the stream's timer still delivers, so nothing hangs.

use std::sync::{Arc, RwLock};

use crate::lifecycle::remote_wake::RunEventWaker;

pub(super) struct RunEventFanout {
    waker: RwLock<Option<Arc<dyn RunEventWaker>>>,
}

impl RunEventFanout {
    pub(super) fn new() -> Self {
        Self {
            waker: RwLock::new(None),
        }
    }

    /// Route every later notification to `waker`. Rebinding replaces.
    pub(super) fn bind(&self, waker: Arc<dyn RunEventWaker>) {
        *self.waker.write().unwrap_or_else(|e| e.into_inner()) = Some(waker);
    }

    fn waker(&self) -> Option<Arc<dyn RunEventWaker>> {
        self.waker.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Deliver one notification. The payload is the run id and nothing else;
    /// anything empty is ignored. Returns whether a stream here was woken.
    pub(super) fn deliver(&self, payload: &str) -> bool {
        let run_id = payload.trim();
        if run_id.is_empty() {
            return false;
        }
        match self.waker() {
            Some(waker) => waker.wake_run(run_id),
            None => false,
        }
    }

    /// The listener (re)connected: notifications sent while it was down are
    /// lost server-side, so every parked stream reads once to catch up —
    /// the stream-side twin of the `Wake::All` the task waiters get.
    pub(super) fn catch_up(&self) {
        if let Some(waker) = self.waker() {
            waker.wake_all_runs();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::lifecycle::remote_wake::{RemoteWakeSubscription, RemoteWakes};

    async fn rang(sub: &RemoteWakeSubscription) -> bool {
        tokio::time::timeout(Duration::from_millis(50), sub.notified())
            .await
            .is_ok()
    }

    #[tokio::test]
    async fn a_payload_wakes_exactly_the_run_it_names() {
        let wakes = Arc::new(RemoteWakes::new());
        let a = wakes.subscribe("run-a");
        let b = wakes.subscribe("run-b");
        let fanout = RunEventFanout::new();
        fanout.bind(wakes.clone());
        assert!(fanout.deliver("run-a"));
        assert!(rang(&a).await);
        assert!(!rang(&b).await);
    }

    #[tokio::test]
    async fn an_unknown_run_and_an_empty_payload_are_ignored() {
        let wakes = Arc::new(RemoteWakes::new());
        let a = wakes.subscribe("run-a");
        let fanout = RunEventFanout::new();
        fanout.bind(wakes.clone());
        assert!(!fanout.deliver("run-z"));
        assert!(!fanout.deliver(""));
        assert!(!fanout.deliver("   "));
        assert!(!rang(&a).await);
    }

    #[tokio::test]
    async fn an_unbound_fanout_drops_the_notification() {
        let fanout = RunEventFanout::new();
        assert!(!fanout.deliver("run-a"));
        fanout.catch_up();
    }

    #[tokio::test]
    async fn a_catch_up_wakes_every_parked_stream() {
        let wakes = Arc::new(RemoteWakes::new());
        let a = wakes.subscribe("run-a");
        let b = wakes.subscribe("run-b");
        let fanout = RunEventFanout::new();
        fanout.bind(wakes.clone());
        fanout.catch_up();
        assert!(rang(&a).await);
        assert!(rang(&b).await);
    }
}
