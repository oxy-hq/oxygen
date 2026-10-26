//! A failed root task retried per the policy on its queue row.
//!
//! The policy a host attaches at enqueue (an Oxy Function's manifest
//! `retries`) is honoured here, at the queue, rather than by the coordinator:
//! the failed claim goes back to `queued` behind its backoff
//! ([`crud::requeue_failed_attempt`]) and the coordinator hears nothing until
//! an attempt succeeds or the last one fails. Nothing sleeps — the run's own
//! scoped worker claims the row again once `available_at` passes — and a
//! retry is claimable only after the failed execution has returned, so two
//! attempts never run side by side.
//!
//! Opt-in per transport ([`DurableTransport::retry_failed_root`]): the
//! pipeline turns it on for the runs whose policy is a user's declared
//! contract. Other roots that carry a policy keep their behaviour.

use std::sync::atomic::Ordering;

use agentic_core::delegation::{ClaimEpoch, NO_CLAIM, TaskOutcome};
use agentic_core::transport::WorkerMessage;

use super::DurableTransport;
use super::claims::fence;
use crate::orchestrator::crud;

impl DurableTransport {
    /// Retry this transport's root task, when it fails, as its queue row's
    /// policy says. See the module docs.
    pub fn retry_failed_root(&self) {
        self.retry_root.store(true, Ordering::Relaxed);
    }

    fn retries(&self, task_id: &str) -> bool {
        self.retry_root.load(Ordering::Relaxed) && self.task_id_root.as_deref() == Some(task_id)
    }

    /// `msg` as it should continue through [`Self::send`], or `None` when the
    /// failure was absorbed by a retry and nothing is to be reported.
    pub(super) async fn retry_or_report(&self, msg: WorkerMessage) -> Option<WorkerMessage> {
        let WorkerMessage::Outcome {
            task_id,
            outcome: TaskOutcome::Failed(error),
            claim_epoch,
        } = &msg
        else {
            return Some(msg);
        };
        if !self.retries(task_id) {
            return Some(msg);
        }
        let error = self.on_failed_attempt(task_id, *claim_epoch, error).await?;
        Some(WorkerMessage::Outcome {
            task_id: task_id.clone(),
            outcome: TaskOutcome::Failed(error),
            claim_epoch: *claim_epoch,
        })
    }

    /// Requeue the failed claim if it has a retry left (`None`), else the
    /// error to report. Every path that cannot requeue reports the failure, so
    /// a DB error ends the run rather than stranding it.
    async fn on_failed_attempt(
        &self,
        task_id: &str,
        claim: ClaimEpoch,
        error: &str,
    ) -> Option<String> {
        let w = self.worker_id.as_str();
        let state = match crud::retry_state(&self.db, task_id, w, fence(claim)).await {
            Ok(Some(state)) => state,
            // The claim is gone: the ordinary stamp reports it as stale.
            Ok(None) => return Some(error.to_string()),
            Err(e) => {
                tracing::error!(target: "transport", %task_id, error = %e,
                    "could not read the task's retry state; failing it");
                return Some(error.to_string());
            }
        };
        let final_error = crud::final_failure_message(state.attempts(), error);
        let Some(delay) = state.next_delay(error) else {
            return Some(final_error);
        };
        // The row goes back to the queue, so this process stops owning the
        // claim — stop proving otherwise before the write, as a deferral does.
        self.retire_heartbeat(task_id, claim, "task failed; retry queued");
        match crud::requeue_failed_attempt(&self.db, task_id, w, fence(claim), state.spent, delay)
            .await
        {
            Ok(true) => {
                tracing::warn!(target: "transport", %task_id, attempt = state.attempts(),
                    delay_ms = delay.as_millis() as u64, %error,
                    "task failed; retrying after its backoff");
                None
            }
            Ok(false) => Some(error.to_string()),
            Err(e) => {
                tracing::error!(target: "transport", %task_id, error = %e,
                    "could not requeue the failed task for its retry; failing it");
                Some(final_error)
            }
        }
    }

    /// A Stop for a task waiting out a retry's backoff: no execution exists
    /// to report it, so report it here. `true` when it was one.
    pub(super) async fn cancel_waiting_retry(&self, task_id: &str) -> bool {
        if !self.retries(task_id) {
            return false;
        }
        match crud::cancel_waiting_retry(&self.db, task_id).await {
            Ok(true) => {
                let msg = WorkerMessage::Outcome {
                    task_id: task_id.to_string(),
                    outcome: TaskOutcome::Cancelled,
                    claim_epoch: NO_CLAIM,
                };
                if self.message_tx.send(msg).await.is_err() {
                    tracing::warn!(target: "transport", %task_id,
                        "stop during a retry's backoff not delivered: the coordinator channel is closed");
                }
                true
            }
            Ok(false) => false,
            Err(e) => {
                tracing::warn!(target: "transport", %task_id, error = %e,
                    "could not check for a waiting retry; cancelling as usual");
                false
            }
        }
    }
}
