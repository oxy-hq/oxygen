//! What a start leaves behind when its caller stops waiting for it.
//!
//! [`crate::PipelineBuilder::start`] inserts the run row — leased to the start
//! (`stamp_starting_lease`) — and then builds the pipeline, which on a cold
//! DuckDB cache under load takes tens of seconds. Its caller is usually an HTTP
//! handler: when the request's `TimeoutLayer` fires or the client disconnects,
//! the handler's future is dropped, and with it the start, before anything
//! spawned a driver. Nothing then ends the run: it stays `running` with a lease
//! nobody renews, no event is ever written, and reopening its thread resumes a
//! stream that never produces one. No periodic pass retires it either — the
//! stranded tick selects only `workflow` and `airway` roots, and the boot
//! cleanup runs once per process start.
//!
//! [`StartGuard`] is armed for the start and disarmed when it returns, whether
//! with a pipeline or an error (the error path marks the run failed itself).
//! Dropped while still armed, it marks the run failed, which also clears the
//! lease.
//!
//! The alternative, running the start on its own task so a dropped request
//! does not stop it, was not taken: the caller has already answered the client
//! with a timeout and no run id, so the run would go on to answer a question
//! the client was told failed, and the handler's time bound would no longer
//! bound the work it starts.

use agentic_core::hub_task::bind_current_hub;
use sea_orm::DatabaseConnection;

/// The `error_message` of a run whose start was dropped: shown to the asker
/// when they reopen the thread.
pub const START_INTERRUPTED: &str = "This question was interrupted before it started — ask again.";

/// Fails the run `start` inserted unless [`Self::disarm`] is called first.
pub(crate) struct StartGuard {
    armed: Option<(DatabaseConnection, String)>,
}

impl StartGuard {
    pub(crate) fn new(db: &DatabaseConnection, run_id: &str) -> Self {
        Self {
            armed: Some((db.clone(), run_id.to_string())),
        }
    }

    /// The start finished; whatever happens to the run now is its caller's.
    pub(crate) fn disarm(mut self) {
        self.armed = None;
    }
}

impl Drop for StartGuard {
    fn drop(&mut self) {
        let Some((db, run_id)) = self.armed.take() else {
            return;
        };
        // `Drop` cannot await: the write goes on a task of its own. A start
        // dropped before its insert finds no row, and the write is a no-op.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(
                run_id,
                "start dropped outside a runtime; its run is left for recovery"
            );
            return;
        };
        runtime.spawn(bind_current_hub(async move {
            match agentic_runtime::crud::update_run_failed(&db, &run_id, START_INTERRUPTED).await {
                Ok(()) => tracing::info!(
                    run_id,
                    "start dropped before its driver was spawned; run marked failed"
                ),
                Err(e) => tracing::debug!(
                    run_id,
                    error = %e,
                    "start dropped; could not mark its run failed (row may not exist)"
                ),
            }
        }));
    }
}
