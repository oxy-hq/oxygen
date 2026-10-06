//! A driver taking back the claim of a task that is parked, not runnable.

use agentic_core::transport::{TransportError, WorkerTransport};

use super::DurableTransport;
use crate::orchestrator::crud;
use crate::orchestrator::worker::HEARTBEAT_INTERVAL;

impl DurableTransport {
    /// Hold the `queued` task `task_id` as this process's claim, and start the
    /// heartbeat that says so. Returns whether there was a `queued` row to
    /// take.
    ///
    /// The row goes straight to `claimed` without being handed to a worker,
    /// so its spec is never executed — see [`crud::adopt_queued_task`] for
    /// when that is the right thing to do with a queued task. The ticker is
    /// the ordinary one ([`WorkerTransport::spawn_heartbeat`]), so everything
    /// that retires a claim's ticker retires this one: a terminal outcome, a
    /// deferral, or the same `task_id` being claimed again once the
    /// coordinator re-assigns it.
    ///
    /// The claim and the ticker belong together and are only offered
    /// together: a `claimed` row nobody beats is requeued by the reaper a
    /// minute later, which is the state this exists to leave.
    pub async fn adopt_queued_claim(&self, task_id: &str) -> Result<bool, TransportError> {
        let adopted = crud::adopt_queued_task(&self.db, task_id, &self.worker_id)
            .await
            .map_err(|e| TransportError::Other(format!("adopting a queued claim failed: {e}")))?;
        if adopted {
            self.spawn_heartbeat(task_id, HEARTBEAT_INTERVAL);
        }
        Ok(adopted)
    }
}
