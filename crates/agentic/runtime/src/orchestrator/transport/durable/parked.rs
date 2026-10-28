//! A driver giving up the claim of a root that is parked on a person.

use super::DurableTransport;
use super::claims::fence;
use crate::orchestrator::crud;

impl DurableTransport {
    /// End this process's claim on `task_id`, which is suspended for human
    /// input and whose suspension is already on record: stamp the row
    /// `completed` and retire its heartbeat.
    ///
    /// A suspended task normally keeps its claim, so the same `task_id` can
    /// resume in place (`internal-docs/worker-fleet.md` § "A suspended task
    /// still owns its claim"). That is what a delegation needs: its children
    /// report back to a coordinator that is still there. A question to a
    /// person is different. Nothing answers it on a clock, so a held claim is
    /// a driver lease, a heartbeat and a cancel poll spent for as long as the
    /// process lives — and when the process goes, the row is requeued with
    /// the spec the run was *started* with, under a run no driver loop selects
    /// (`awaiting_input` is not a pending-Global status). Ended here, the run
    /// is left as its suspension describes it, and an answer continues it from
    /// that checkpoint rather than through this claim.
    ///
    /// Only the claim this process is beating is ended, fenced on its epoch:
    /// a row another process (or this one, re-claiming) holds is never
    /// written. Returns what the write found.
    pub async fn release_parked_claim(
        &self,
        task_id: &str,
    ) -> Result<crud::TerminalWrite, sea_orm::DbErr> {
        let Some(claim) = self.heartbeats.held(task_id) else {
            return Ok(crud::TerminalWrite::NotOwned);
        };
        let db = &self.db;
        let w = self.worker_id.as_str();
        self.stamp_terminal(task_id, claim, "parked on a person", || {
            crud::complete_queue_task(db, task_id, w, fence(claim))
        })
        .await
    }
}
