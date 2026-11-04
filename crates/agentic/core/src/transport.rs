//! Transport traits for the coordinator-worker architecture.
//!
//! The coordinator publishes [`TaskAssignment`]s and receives
//! [`WorkerMessage`]s. Workers pull assignments and send messages back.
//! The transport layer is swappable: in-process channels for day 1,
//! gRPC or message queues for distributed deployment later.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::delegation::{ClaimEpoch, TaskAssignment, TaskOutcome};

// ── WorkerMessage ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerMessage {
    /// A serialized event from the running task.
    Event {
        task_id: String,
        event_type: String,
        payload: Value,
    },
    /// The task produced a final outcome.
    ///
    /// `claim_epoch` is the claim of the execution that produced it, so a
    /// durable transport only ever writes to the row that execution held.
    Outcome {
        task_id: String,
        outcome: TaskOutcome,
        #[serde(default)]
        claim_epoch: ClaimEpoch,
    },
    /// The task could not run yet — return it to the queue, invisible for
    /// `delay_secs`.
    ///
    /// Distinct from `Outcome` on purpose: a deferral produces no result, so
    /// recording one would make the coordinator believe the task ran. The
    /// durable transport turns this into the queue's delayed-visibility
    /// write; in-memory transports may simply drop it.
    Defer {
        task_id: String,
        delay_secs: u64,
        max_wait_secs: u64,
        reason: String,
        /// The deferring execution's claim; see `Outcome::claim_epoch`.
        #[serde(default)]
        claim_epoch: ClaimEpoch,
    },
}

// ── TransportError ───────────────────────────────────────────────────────────

/// Errors that can occur during transport operations.
#[derive(Debug)]
pub enum TransportError {
    /// The underlying channel has been closed.
    ChannelClosed,
    /// The referenced task was not found.
    NotFound,
    /// Any other transport-level error.
    Other(String),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::ChannelClosed => write!(f, "transport channel closed"),
            TransportError::NotFound => write!(f, "task not found"),
            TransportError::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for TransportError {}

// ── CoordinatorTransport ─────────────────────────────────────────────────────

/// Coordinator-side transport: sends assignments to workers, receives messages.
#[async_trait]
pub trait CoordinatorTransport: Send + Sync + 'static {
    /// Send a task assignment for a worker to pick up.
    async fn assign(&self, assignment: TaskAssignment) -> Result<(), TransportError>;

    /// Receive the next message from any worker.
    ///
    /// Returns `None` when all worker-side senders have been dropped.
    async fn recv(&self) -> Option<WorkerMessage>;

    /// Signal cancellation of a specific task.
    async fn cancel(&self, task_id: &str) -> Result<(), TransportError>;

    /// Signal cancellation of a task **and all of its descendants**.
    ///
    /// Descendant task ids are derived from the hierarchical `parent.child`
    /// id format produced by the coordinator (e.g. `abc.1`, `abc.1.2`). An
    /// implementation may match descendants by prefix on the set of live
    /// cancellation tokens.
    ///
    /// Default implementation only cancels the root — override to propagate.
    async fn cancel_subtree(&self, root_task_id: &str) -> Result<(), TransportError> {
        self.cancel(root_task_id).await
    }
}

// ── WorkerTransport ──────────────────────────────────────────────────────────

/// Worker-side transport: pulls assignments, sends messages back.
#[async_trait]
pub trait WorkerTransport: Send + Sync + 'static {
    /// Pull the next task assignment from the coordinator.
    ///
    /// # Contract
    ///
    /// An implementation **must eventually return `None`** once no further
    /// assignment can arrive. `Worker::run` breaks only on `None`, so a
    /// transport that can never produce one turns every spawned worker into a
    /// task that outlives the run it was spawned for — and, for a durable
    /// transport, into a standing database poller.
    ///
    /// This is a contract, not a description of any one mechanism, and the
    /// distinction has already cost us an outage. The previous wording —
    /// "returns `None` when the coordinator-side sender has been dropped" —
    /// described only the channel-backed case. `DurableTransport` polls a
    /// table and has no sender to drop, so it satisfied that sentence
    /// *vacuously* while never returning `None` at all. Substituting it under
    /// a per-run worker spawn (#2034) silently removed the worker's only exit,
    /// leaked one poller per run driven for 139 days, and exhausted the
    /// Postgres connection ceiling in production on 2026-09-01 (#3059).
    ///
    /// A transport with no natural end-of-stream must therefore carry an
    /// explicit retirement signal and return `None` when it fires — as
    /// `DurableTransport::retire_worker_loop` now does. If you are adding a
    /// transport and cannot name the condition under which this returns
    /// `None`, it does not satisfy this contract.
    async fn recv_assignment(&self) -> Option<TaskAssignment>;

    /// Pull the next assignment holding one of `slots` — the process's worker
    /// concurrency cap — for it. `None` exactly when [`Self::recv_assignment`]
    /// would return `None`.
    ///
    /// The slot is the caller's to release (drop) when the task stops
    /// occupying it.
    ///
    /// The default receives first and waits for a slot second. That is right
    /// for a channel transport, where a received assignment holds nothing
    /// while it waits. A transport whose receive *claims* something — a row
    /// lock, a lease that must be heartbeated — must override this to take
    /// the slot **before** it claims, and to hold no slot while it has nothing
    /// to claim: a claim waiting behind a slot holds its lease for nothing,
    /// and an idle receiver holding a slot starves every other caller.
    async fn recv_admitted(
        &self,
        slots: &Arc<Semaphore>,
    ) -> Option<(TaskAssignment, OwnedSemaphorePermit)> {
        let assignment = self.recv_assignment().await?;
        let slot = Arc::clone(slots).acquire_owned().await.ok()?;
        Some((assignment, slot))
    }

    /// Send a message (event or outcome) back to the coordinator.
    async fn send(&self, msg: WorkerMessage) -> Result<(), TransportError>;

    /// Get a cancellation token for a specific task.
    ///
    /// The token is cancelled when the coordinator calls `cancel(task_id)`.
    ///
    /// `claim_epoch` is the executing claim ([`crate::delegation::NO_CLAIM`]
    /// for work with no queue row). A durable transport keys tokens by it, so
    /// cancelling one claim never cancels its re-claim.
    fn cancellation_token(&self, task_id: &str, claim_epoch: ClaimEpoch) -> CancellationToken;

    /// Send a heartbeat for a running task.
    ///
    /// Durable transports update a heartbeat timestamp in the database;
    /// in-memory transports no-op. Workers call this periodically to prevent
    /// the reaper from reclaiming their tasks.
    async fn heartbeat(
        &self,
        _task_id: &str,
        _claim_epoch: ClaimEpoch,
    ) -> Result<(), TransportError> {
        Ok(())
    }

    /// Spawn a background heartbeat loop for a task.
    ///
    /// Returns a [`CancellationToken`] — cancel it when the task completes.
    /// Durable transports periodically update the heartbeat timestamp;
    /// in-memory transports return a no-op token.
    fn spawn_heartbeat(
        &self,
        _task_id: &str,
        _claim_epoch: ClaimEpoch,
        _interval: std::time::Duration,
    ) -> CancellationToken {
        CancellationToken::new()
    }
}
