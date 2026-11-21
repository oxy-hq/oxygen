//! The heartbeat ticker of one claim.

use std::time::Duration;

use agentic_core::delegation::ClaimEpoch;
use agentic_core::hub_task::spawn_with_hub;
use sea_orm::DatabaseConnection;
use tokio_util::sync::CancellationToken;

use super::claims::fence;
use super::{is_shutting_down, shutdown_signal};
use crate::crud;

/// What one heartbeat ticker needs to act on the result of a beat.
pub(super) struct BeatCtx {
    pub(super) task_id: String,
    pub(super) claim: ClaimEpoch,
    /// The token `spawn_heartbeat` returned; cancelled = this ticker retired.
    pub(super) ticker: CancellationToken,
    /// This claim's execution token.
    pub(super) task_token: CancellationToken,
    pub(super) beat_timeout: Duration,
}

type BeatResult =
    Result<Result<crud::HeartbeatOutcome, sea_orm::DbErr>, tokio::time::error::Elapsed>;

impl BeatCtx {
    /// Act on one beat's result. `true` means stop ticking.
    fn should_stop(&self, beat: BeatResult) -> bool {
        let task_id = &self.task_id;
        match beat {
            Ok(Ok(crud::HeartbeatOutcome::Beat)) => false,
            // No queue row at all: an interactive root registered without
            // one. Nothing to keep alive and nothing lost.
            Ok(Ok(crud::HeartbeatOutcome::NoRow)) => {
                tracing::debug!(%task_id, "heartbeat: no queue row for this task; stopping ticker");
                true
            }
            Ok(Ok(crud::HeartbeatOutcome::Lost)) => {
                self.on_lost_claim();
                true
            }
            // This claim's own terminal write landed before its outcome
            // retired the ticker: the work is done, so stop without a cancel.
            Ok(Ok(crud::HeartbeatOutcome::Settled)) => {
                tracing::debug!(%task_id, claim = self.claim, "heartbeat: claim settled by its own terminal write; stopping ticker");
                true
            }
            // A failed beat is not a lost claim. Keep ticking: the next one
            // may land inside the visibility window, and a dead ticker
            // guarantees it won't. Expected once the co-located DB goes down
            // on shutdown; a real failure otherwise.
            Ok(Err(e)) if is_shutting_down() => {
                tracing::debug!(%task_id, "heartbeat failed during shutdown: {e}");
                false
            }
            Ok(Err(e)) => {
                tracing::warn!(%task_id, "heartbeat failed, will retry next tick: {e}");
                false
            }
            Err(_elapsed) => {
                tracing::warn!(
                    %task_id,
                    "heartbeat timed out after {:?}, will retry next tick",
                    self.beat_timeout
                );
                false
            }
        }
    }

    /// The predicate carries this claim's epoch, so `Lost` means *this
    /// execution's* claim is gone: reaped, released, taken by a peer, cancelled
    /// by a Stop from another process, or superseded by a re-claim in this very
    /// process. A ticker retired first (terminal outcome, deferral) just stops;
    /// otherwise the task keeps running on a claim it no longer holds, so stop
    /// it too, or it runs twice. Only this claim's token is cancelled — a
    /// re-claim has its own.
    fn on_lost_claim(&self) {
        let task_id = &self.task_id;
        if self.ticker.is_cancelled() {
            tracing::debug!(%task_id, "heartbeat: claim no longer held; stopping ticker");
            return;
        }
        tracing::warn!(%task_id, claim = self.claim, "heartbeat: claim lost under a running task; cancelling it");
        self.task_token.cancel();
    }
}

/// Beat `ctx`'s claim every `interval` until it is retired, lost, or the
/// process shuts down.
pub(super) fn spawn_ticker(
    db: DatabaseConnection,
    worker_id: String,
    ctx: BeatCtx,
    interval: Duration,
) {
    spawn_with_hub(async move {
        let mut ticker = tokio::time::interval(interval);
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    // Race the heartbeat write against shutdown too: an
                    // in-flight write when the co-located DB dies would
                    // otherwise block the full pool acquire timeout. On
                    // shutdown, stop the ticker — there is nothing left to
                    // keep alive.
                    let beat = tokio::select! {
                        _ = shutdown_signal().cancelled() => break,
                        r = tokio::time::timeout(
                            ctx.beat_timeout,
                            crud::heartbeat_queue_task(&db, &ctx.task_id, &worker_id, fence(ctx.claim)),
                        ) => r,
                    };
                    if ctx.should_stop(beat) {
                        break;
                    }
                }
                _ = ctx.ticker.cancelled() => break,
                // Idle between ticks: abandon at once rather than waiting a
                // full interval to notice shutdown.
                _ = shutdown_signal().cancelled() => break,
            }
        }
        // Whatever ended the loop, the ticker is no longer proving the
        // claim. Say so on the token `spawn_heartbeat` returned, so a caller (or
        // a test) can tell a live ticker from a dead one.
        ctx.ticker.cancel();
    });
}
