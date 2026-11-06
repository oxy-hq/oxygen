//! What an `oxy_task_enqueued` notification says, and whom it wakes.
//!
//! The queue's NOTIFY trigger sends `task:<task_id>` for the row that became
//! `queued` (`migration::task_queue_notify_payload`). A worker scoped to one
//! run's tree can only claim rows with `root_task_id = root`
//! (`crud::claim_task_under_root`) — for every production row, `task_id =
//! root` or `<root>.…`, which is what the match here tests — so a wake for any other task would send it
//! to the queue for nothing — and every run's poller in every process used to
//! do exactly that on every enqueue fleet-wide.
//!
//! Anything that does not name a task wakes everyone: the empty payload an
//! older trigger (or a peer mid-deploy) sends, a payload this code cannot
//! parse, a (re)connect. A dropped wake costs a poll interval; a spurious one
//! costs one claim query.

use std::sync::Arc;

use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;

/// The payload prefix the trigger puts before the task id.
pub(super) const TASK_PAYLOAD_PREFIX: &str = "task:";

/// Wakes buffered per waiter. A waiter that falls further behind than this
/// wakes anyway (it cannot tell whether one it missed was its own).
pub(super) const WAKE_BUFFER: usize = 1024;

/// One wake from the listener.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Wake {
    /// Unattributable: everyone looks.
    All,
    /// A task became claimable; only pollers whose scope covers it look.
    Task(Arc<str>),
}

impl Wake {
    pub(super) fn from_payload(payload: &str) -> Self {
        match payload.strip_prefix(TASK_PAYLOAD_PREFIX) {
            Some(task_id) if !task_id.is_empty() => Wake::Task(Arc::from(task_id)),
            _ => Wake::All,
        }
    }

    /// Does this wake concern a poller scoped to `root` (`None`: unscoped)?
    pub(super) fn concerns(&self, root: Option<&str>) -> bool {
        match (self, root) {
            (Wake::All, _) | (_, None) => true,
            (Wake::Task(task_id), Some(root)) => under_root(task_id, root),
        }
    }
}

/// The scoped claim's predicate, minus `LIKE`'s wildcards.
fn under_root(task_id: &str, root: &str) -> bool {
    task_id
        .strip_prefix(root)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
}

/// Return at the next wake that concerns `root`.
pub(super) async fn next_wake_for(wakes: &mut broadcast::Receiver<Wake>, root: Option<&str>) {
    loop {
        match wakes.recv().await {
            Ok(wake) if wake.concerns(root) => return,
            Ok(_) => {}
            // Missed some; one of them may have been ours.
            Err(RecvError::Lagged(_)) => return,
            // The router is gone: nothing will ever come. Leave it to the
            // caller's backstop rather than spin.
            Err(RecvError::Closed) => std::future::pending::<()>().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_task_payload_names_its_task() {
        assert_eq!(
            Wake::from_payload("task:r1.2"),
            Wake::Task(Arc::from("r1.2"))
        );
    }

    #[test]
    fn anything_else_wakes_everyone() {
        for payload in ["", "io_bound", "task:", "TASK:r1", " task:r1"] {
            assert_eq!(Wake::from_payload(payload), Wake::All, "{payload:?}");
        }
    }

    #[test]
    fn a_scoped_poller_wakes_for_its_root_and_descendants_only() {
        let wake = |id: &str| Wake::Task(Arc::from(id));
        assert!(wake("r1").concerns(Some("r1")));
        assert!(wake("r1.3").concerns(Some("r1")));
        assert!(wake("r1.3.1").concerns(Some("r1")));
        assert!(!wake("r10").concerns(Some("r1")));
        assert!(!wake("r10.1").concerns(Some("r1")));
        assert!(!wake("r2.1").concerns(Some("r1")));
        assert!(!wake("r").concerns(Some("r1")));
    }

    #[test]
    fn unscoped_pollers_and_unattributed_wakes_always_match() {
        assert!(Wake::Task(Arc::from("r2.1")).concerns(None));
        assert!(Wake::All.concerns(Some("r1")));
    }

    #[tokio::test]
    async fn a_lagged_waiter_wakes() {
        let (tx, mut rx) = broadcast::channel(1);
        tx.send(Wake::Task(Arc::from("other"))).unwrap();
        tx.send(Wake::Task(Arc::from("other.1"))).unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            next_wake_for(&mut rx, Some("mine")),
        )
        .await
        .expect("a lagged receiver must wake");
    }

    #[tokio::test]
    async fn another_roots_wake_is_skipped() {
        let (tx, mut rx) = broadcast::channel(8);
        tx.send(Wake::Task(Arc::from("other.1"))).unwrap();
        let woke = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            next_wake_for(&mut rx, Some("mine")),
        )
        .await;
        assert!(woke.is_err(), "woke for another root's task");
        tx.send(Wake::Task(Arc::from("mine.4"))).unwrap();
        next_wake_for(&mut rx, Some("mine")).await;
    }
}
