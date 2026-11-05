//! The claim loop behind [`WorkerTransport::recv_assignment`] and
//! [`WorkerTransport::recv_admitted`].
//!
//! With a slot pool (`recv_admitted`, which every `Worker` uses) a slot is
//! taken **before** each claim attempt and given back the moment the attempt
//! finds nothing, so:
//!
//! - no row is ever claimed while no slot is free — a claim waiting behind a
//!   slot would hold its lease and heartbeat for nothing;
//! - a poller with nothing to claim holds no slot while it waits for a wake,
//!   so an idle run never starves a busy one of the process cap.
//!
//! Waiting for a slot is FIFO (`tokio::sync::Semaphore` is fair) and each
//! run's worker queues for one slot at a time, so under saturation slots go
//! round-robin across runs: a wide fan-out gets one slot per turn, not all of
//! them.
//!
//! [`WorkerTransport::recv_assignment`]: agentic_core::transport::WorkerTransport::recv_assignment
//! [`WorkerTransport::recv_admitted`]: agentic_core::transport::WorkerTransport::recv_admitted

use std::sync::Arc;

use agentic_core::delegation::{TaskAssignment, TaskSpec};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::{
    DurableTransport, is_pool_acquire_failure, is_shutting_down, pool_starvation_backoff,
    shutdown_signal,
};
use crate::orchestrator::crud;
use crate::orchestrator::entity::task_queue;

/// What one pass of the loop decided.
enum Step {
    /// Retired: the worker loop ends.
    Exit,
    /// Go round again (a shutdown signal, a row handed back, a bad spec).
    Again,
}

impl DurableTransport {
    /// Claim the next task under this transport's scope, holding a slot from
    /// `slots` for it when given. See the module docs.
    pub(super) async fn recv_inner(
        &self,
        slots: Option<&Arc<Semaphore>>,
    ) -> Option<(TaskAssignment, Option<OwnedSemaphorePermit>)> {
        // Consecutive pool-acquire failures, driving the backoff in the `Err`
        // arm below. Reset the moment a claim round-trip succeeds.
        let mut pool_starvation_streak: u32 = 0;
        loop {
            // The ONLY exit. Checked first so a retired transport stops asking
            // the database anything at all.
            if self.worker_loop_cancel.is_cancelled() {
                tracing::debug!(
                    worker_id = %self.worker_id,
                    task_id_root = ?self.task_id_root,
                    "worker loop retired; no longer polling the queue"
                );
                return None;
            }
            // Refuse to take on new work once this process has begun shutting
            // down: graceful release flips our claims `claimed -> queued`,
            // which fires the queue's NOTIFY trigger and wakes *this* loop. See
            // [`super::begin_shutdown`]. Park rather than spin.
            if is_shutting_down() {
                tokio::time::sleep(self.poll_interval).await;
                continue;
            }
            let slot = match slots {
                None => None,
                Some(slots) => match self.take_slot(slots).await {
                    Ok(slot) => Some(slot),
                    Err(Step::Exit) => return None,
                    Err(Step::Again) => continue,
                },
            };
            // Race the claim against the shutdown signal: a poll in flight when
            // a co-located DB dies on shutdown would otherwise block the full
            // pool `ACQUIRE_TIMEOUT` (~30s). `begin_shutdown` sets the gate
            // above before cancelling this signal.
            let claim_result = tokio::select! {
                _ = shutdown_signal().cancelled() => continue,
                r = self.claim_once() => r,
            };
            match claim_result {
                Ok(Some(entry)) => {
                    pool_starvation_streak = 0;
                    if let Some(assignment) = self.admit_claimed(entry).await {
                        return Some((assignment, slot));
                    }
                }
                Ok(None) => {
                    pool_starvation_streak = 0;
                    // Nothing to claim: hold no slot while we wait.
                    drop(slot);
                    self.wait_for_wake().await;
                }
                Err(e) => {
                    drop(slot);
                    self.back_off_after_claim_error(e, &mut pool_starvation_streak)
                        .await;
                }
            }
        }
    }

    /// Wait for a slot, abandoning the wait on retirement or shutdown.
    async fn take_slot(&self, slots: &Arc<Semaphore>) -> Result<OwnedSemaphorePermit, Step> {
        tokio::select! {
            _ = self.worker_loop_cancel.cancelled() => Err(Step::Exit),
            _ = shutdown_signal().cancelled() => Err(Step::Again),
            // Errs only if the semaphore is closed, which nothing does; treat
            // it as the end of the worker rather than spin on it.
            slot = Arc::clone(slots).acquire_owned() => slot.map_err(|_| Step::Exit),
        }
    }

    /// One claim attempt, under this transport's root — see `task_id_root`
    /// for why. An unscoped transport claims nothing: no production worker
    /// drains one, and the global claim it once made had no other caller.
    async fn claim_once(&self) -> Result<Option<task_queue::Model>, sea_orm::DbErr> {
        let Some(root) = &self.task_id_root else {
            tracing::warn!(
                worker_id = %self.worker_id,
                "unscoped durable transport polled for work; it claims nothing"
            );
            return Ok(None);
        };
        crud::claim_task_under_root(&self.db, &self.worker_id, root).await
    }

    /// Turn a claimed row into an assignment, or hand it back and return
    /// `None`.
    async fn admit_claimed(&self, entry: task_queue::Model) -> Option<TaskAssignment> {
        // Close the gate-to-claim race at its cause: shutdown can begin while
        // the claim is in flight, and the release pass can then run before this
        // row lands `claimed`. This is the only point that knows both that the
        // claim exists and that the process is leaving.
        if is_shutting_down() {
            self.hand_back_during_shutdown(&entry).await;
            return None;
        }
        let spec: TaskSpec = match serde_json::from_value(entry.spec) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(task_id = %entry.task_id, "failed to deserialize task spec: {e}");
                // We hold the claim — the UPDATE just granted it — so the
                // ownership predicate matches.
                let _ = crud::fail_queue_task(
                    &self.db,
                    &entry.task_id,
                    &self.worker_id,
                    Some(entry.claim_epoch),
                )
                .await;
                return None;
            }
        };
        let policy = entry.policy.and_then(|p| {
            serde_json::from_value(p)
                .map_err(|e| {
                    tracing::warn!(task_id = %entry.task_id, "failed to deserialize task policy: {e}");
                })
                .ok()
        });
        Some(TaskAssignment {
            task_id: entry.task_id,
            parent_task_id: entry.parent_task_id,
            run_id: entry.run_id,
            spec,
            policy,
            claim_epoch: entry.claim_epoch,
        })
    }

    async fn hand_back_during_shutdown(&self, entry: &task_queue::Model) {
        match crud::release_claim(
            &self.db,
            &entry.task_id,
            &self.worker_id,
            Some(entry.claim_epoch),
        )
        .await
        {
            Ok(_) => tracing::info!(
                task_id = %entry.task_id,
                "claim landed during shutdown; handed straight back to the queue"
            ),
            Err(e) => tracing::warn!(
                task_id = %entry.task_id,
                "failed to hand back a claim taken during shutdown: {e} \
                 (the reaper will reclaim it after the visibility timeout)"
            ),
        }
    }

    /// Nothing claimable: wait for a same-process `assign()`, a cross-process
    /// NOTIFY that can concern this scope, the backstop, or retirement.
    ///
    /// The router is told this transport's root, so an enqueue under another
    /// run's root does not send this poller back to the queue. Only the
    /// router's own backstop bounds the wait — a separate sleep here would
    /// double it.
    async fn wait_for_wake(&self) {
        tokio::select! {
            _ = self.new_task_notify.notified() => {}
            _ = self.router.wait_for_task_under(self.task_id_root.as_deref(), self.poll_interval) => {}
            // An idle poller is parked here, so retirement must land here too.
            _ = self.worker_loop_cancel.cancelled() => {}
        }
    }

    async fn back_off_after_claim_error(&self, e: sea_orm::DbErr, streak: &mut u32) {
        // A DB failure while shutting down is expected: the co-located
        // database is going down with us.
        if is_shutting_down() {
            tracing::debug!("failed to claim task from queue during shutdown: {e}");
            tokio::time::sleep(self.poll_interval).await;
        } else if is_pool_acquire_failure(&e) {
            // The pool is starved, and this poll already spent the full
            // `ACQUIRE_TIMEOUT` finding that out. sqlx's waiter queue is FIFO,
            // so a claimer that re-queues at once sits permanently ahead of
            // request traffic (2026-09-01). Back off so a starved pool drains
            // toward the callers a human is waiting on.
            *streak = streak.saturating_add(1);
            let backoff = pool_starvation_backoff(*streak, self.backoff_offset_ms);
            tracing::error!(
                worker_id = %self.worker_id,
                streak = *streak,
                backoff_ms = backoff.as_millis() as u64,
                "failed to claim task from queue: {e} (pool starved; backing off \
                 so request traffic is not queued behind this poller)"
            );
            tokio::time::sleep(backoff).await;
        } else {
            tracing::error!("failed to claim task from queue: {e}");
            tokio::time::sleep(self.poll_interval).await;
        }
    }
}
