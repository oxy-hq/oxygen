//! Transport-agnostic runtime state for managing in-flight pipeline runs.

use std::sync::Arc;

use agentic_core::HumanInputQuestion;
use dashmap::DashMap;
use sea_orm::DatabaseConnection;
use tokio::sync::{Notify, mpsc, watch};

use crate::lifecycle::crud;
use crate::lifecycle::remote_wake::RemoteWakes;

/// Shared state for all in-flight pipeline runs.
///
/// Transport-agnostic: no HTTP types, no axum. Can be used from HTTP, gRPC,
/// CLI, or tests.
pub struct RuntimeState {
    /// Woken when new events are written for a run — by the driver in THIS
    /// process. See [`remote_wakes`](Self::remote_wakes) for the other pods.
    pub notifiers: DashMap<String, Arc<Notify>>,
    /// Woken when ANY process appends events for a run: the write path pairs
    /// each append with a `NOTIFY oxy_run_events`, and the task router's
    /// listener fans the run id in here. Only a stream whose driver may be
    /// elsewhere subscribes, so a locally-driven stream is never woken twice
    /// for one flush ([`crate::lifecycle::remote_wake`]).
    pub remote_wakes: Arc<RemoteWakes>,
    /// Delivers user answers to suspended pipeline tasks (HITL).
    pub answer_txs: DashMap<String, mpsc::Sender<String>>,
    /// Cancellation signals for running pipelines.
    pub cancel_txs: DashMap<String, watch::Sender<bool>>,
    /// In-memory run status cache.
    pub statuses: DashMap<String, RunStatus>,
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeState {
    pub fn new() -> Self {
        Self {
            notifiers: DashMap::new(),
            remote_wakes: Arc::new(RemoteWakes::new()),
            answer_txs: DashMap::new(),
            cancel_txs: DashMap::new(),
            statuses: DashMap::new(),
        }
    }

    /// Register a new active run; called before spawning the pipeline task.
    pub fn register(
        &self,
        run_id: &str,
        answer_tx: mpsc::Sender<String>,
        cancel_tx: watch::Sender<bool>,
    ) {
        self.notifiers
            .insert(run_id.to_string(), Arc::new(Notify::new()));
        self.answer_txs.insert(run_id.to_string(), answer_tx);
        self.cancel_txs.insert(run_id.to_string(), cancel_tx);
        self.statuses.insert(run_id.to_string(), RunStatus::Running);
    }

    /// Register just the notifier for a child delegation run (no HITL channels).
    ///
    /// Unlike `register`, does not create answer/cancel channels — those are only
    /// needed for runs that can receive human input or be cancelled by the user.
    /// Child delegation runs are auto-accepted and cancelled via the coordinator.
    pub fn register_notifier(&self, run_id: &str) {
        self.notifiers
            .insert(run_id.to_string(), Arc::new(Notify::new()));
        self.statuses.insert(run_id.to_string(), RunStatus::Running);
    }

    /// Remove the notifier for a completed child delegation run.
    ///
    /// The SSE subscriber exits when it sees the terminal event, so the notifier
    /// can be removed safely once the child run is done/failed/cancelled.
    pub fn deregister_notifier(&self, run_id: &str) {
        self.notifiers.remove(run_id);
        // Leave status so late subscribers can read it.
    }

    /// Remove all in-memory state for a completed (done/failed) run.
    pub fn deregister(&self, run_id: &str) {
        self.notifiers.remove(run_id);
        self.answer_txs.remove(run_id);
        self.cancel_txs.remove(run_id);
        // Leave status so late subscribers can read it.
    }

    /// Clean up channels for a suspended run while keeping notifiers and
    /// status alive for SSE subscribers.
    ///
    /// Called when the coordinator exits with a suspended root task (e.g.,
    /// after timeout). The notifier stays alive so SSE can deliver the
    /// `input_resolved` event when the run is later resumed.
    pub fn suspend_cleanup(&self, run_id: &str) {
        self.answer_txs.remove(run_id);
        self.cancel_txs.remove(run_id);
    }

    /// Wake subscribers waiting on new events for this run.
    ///
    /// Uses `notify_one()` (not `notify_waiters()`) so the permit is stored
    /// when no waiter is currently parked.
    pub fn notify(&self, run_id: &str) {
        if let Some(n) = self.notifiers.get(run_id) {
            n.notify_one();
        }
    }

    /// Signal a running pipeline task to cancel; returns false if the run is
    /// not active.
    ///
    /// Sets the in-memory `statuses` entry to [`RunStatus::Cancelled`]
    /// *before* signalling the pipeline. This ordering matters for runs
    /// suspended on delegation: the cancel token gets propagated to active
    /// children by the cancel forwarder, but if the children finish before
    /// that propagation completes, the coordinator's `record_child_result`
    /// still sees the root as `Cancelled` and short-circuits the parent
    /// resume instead of rebuilding and re-running the pipeline the user
    /// just tried to stop.
    pub fn cancel(&self, run_id: &str) -> bool {
        self.statuses
            .insert(run_id.to_string(), RunStatus::Cancelled);
        if let Some(tx) = self.cancel_txs.get(run_id) {
            tx.send(true).is_ok()
        } else {
            false
        }
    }

    /// Cancel all active runs on graceful shutdown and mark them as
    /// `"shutdown"` so the recovery pipeline can resume them on restart.
    ///
    /// Returns the number of runs marked for shutdown. Unlike user-initiated
    /// cancel (which marks as "cancelled"/"failed" — not resumable), shutdown
    /// marks as "shutdown" which the recovery pipeline treats as resumable.
    ///
    /// This is the lifecycle half of process shutdown only. Releasing this
    /// process's durable-queue claims (`orchestrator::crud::queue::
    /// release_claims_for_worker`) is a separate, orchestrator-layer concern
    /// and is the caller's responsibility to invoke after this returns — see
    /// `spawn_shutdown_hook` in `oxy-app`'s `server::router::recovery` and the
    /// `oxy worker` shutdown sequence in `cli::commands::worker`, both of
    /// which may legitimately depend on both layers.
    ///
    /// Only a run that is still unfinished in the database is marked. A
    /// registered run may already be `done`/`failed`/`cancelled`/`timed_out`
    /// (its drive ended but its entry outlived it); writing `shutdown` over
    /// that sends the next process to resume a finished run, find no
    /// checkpoint, and record it `failed`.
    pub async fn shutdown_all(&self, db: &sea_orm::DatabaseConnection) -> usize {
        let mut shutdown_count = 0;
        let run_ids: Vec<String> = self.cancel_txs.iter().map(|e| e.key().clone()).collect();

        for run_id in &run_ids {
            match mark_shutdown_if_unfinished(db, run_id).await {
                Ok(true) => shutdown_count += 1,
                Ok(false) => {}
                Err(e) => tracing::warn!(
                    target: "runtime",
                    run_id,
                    error = %e,
                    "failed to mark run as shutdown"
                ),
            }

            // Cancel the pipeline.
            if let Some(tx) = self.cancel_txs.get(run_id) {
                tx.send(true).ok();
            }
        }

        if shutdown_count > 0 {
            tracing::info!(
                target: "runtime",
                shutdown_count,
                "marked active runs for shutdown (resumable on restart)"
            );
        }
        shutdown_count
    }

    // ── Transport-agnostic operations ────────────────────────────────────

    pub async fn submit_answer(&self, run_id: &str, answer: String) -> Result<(), RunError> {
        let status = self
            .statuses
            .get(run_id)
            .map(|s| s.clone())
            .ok_or(RunError::NotFound)?;

        if !matches!(status, RunStatus::Suspended { .. }) {
            return Err(RunError::NotSuspended);
        }

        let tx = self
            .answer_txs
            .get(run_id)
            .map(|t| t.clone())
            .ok_or(RunError::ChannelClosed)?;

        tx.send(answer).await.map_err(|_| RunError::ChannelClosed)
    }

    /// Cancel a running or suspended run.
    ///
    /// If the in-memory task is still alive, sends the cancel signal. Otherwise
    /// marks the run as failed in the database so clients don't see a perpetual
    /// loading state.
    pub async fn cancel_run(&self, db: &DatabaseConnection, run_id: &str) -> Result<(), RunError> {
        if self.cancel(run_id) {
            return Ok(());
        }
        // Task already gone — mark failed in DB.
        crud::update_run_failed(db, run_id, "cancelled by user")
            .await
            .map_err(RunError::Db)?;
        self.statuses.insert(
            run_id.to_string(),
            RunStatus::Failed("cancelled by user".into()),
        );
        Ok(())
    }
}

/// Mark `run_id` `shutdown` unless it is already finished. `Ok(true)` when the
/// row was marked. A NULL `task_status` reads as running, so it is marked too.
async fn mark_shutdown_if_unfinished(
    db: &DatabaseConnection,
    run_id: &str,
) -> Result<bool, sea_orm::DbErr> {
    use crate::lifecycle::entity::run;
    use sea_orm::{ColumnTrait, Condition, EntityTrait, QueryFilter, sea_query::Expr};

    let res =
        run::Entity::update_many()
            .col_expr(run::Column::TaskStatus, Expr::value("shutdown"))
            .col_expr(
                run::Column::UpdatedAt,
                Expr::value(chrono::Utc::now().fixed_offset()),
            )
            .filter(run::Column::Id.eq(run_id))
            .filter(Condition::any().add(run::Column::TaskStatus.is_null()).add(
                run::Column::TaskStatus.is_not_in(["done", "failed", "cancelled", "timed_out"]),
            ))
            .exec(db)
            .await?;
    Ok(res.rows_affected > 0)
}

/// Current state of a pipeline run (in-memory cache).
#[derive(Debug, Clone)]
pub enum RunStatus {
    Running,
    Suspended { questions: Vec<HumanInputQuestion> },
    Done,
    Failed(String),
    Cancelled,
}

impl RunStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            RunStatus::Done | RunStatus::Failed(_) | RunStatus::Cancelled
        )
    }
}

/// Error type for transport-agnostic run operations.
#[derive(Debug)]
pub enum RunError {
    NotFound,
    NotSuspended,
    ChannelClosed,
    Db(sea_orm::DbErr),
}
