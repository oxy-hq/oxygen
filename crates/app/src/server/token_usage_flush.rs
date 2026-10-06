//! Write the in-process token-usage counters to `api_token_usage_daily` once a
//! minute, and once more on graceful shutdown (API-tokens design §3.7).
//!
//! **Why an in-process loop and not a `TaskSpec`.** The work is "flush *this
//! process's* memory": the counters live in `oxy_auth::token::usage`'s
//! accumulator, so a task claimed by another worker would have nothing to
//! write. It is also deliberately not durable — a lost minute of counts is the
//! stated trade for not writing a row per request — which is the one class of
//! periodic work the task queue is not for. Every HTTP replica runs its own;
//! the upsert is additive, so they sum. The retention prune, which *is* global
//! and idempotent, rides the daily audit prune loop instead
//! (`oxy_app_core::audit::spawn_audit_prune_loop`).
//!
//! Best-effort throughout: a failed flush is logged and its rows stay in the
//! accumulator for the next tick.

use oxy::database::client::establish_connection;
use oxy_auth::token::usage;
use tokio_util::sync::CancellationToken;

/// Spawn the flusher. The returned handle finishes after the final flush, so
/// the serve command can wait for it on the way out.
pub(crate) fn spawn_token_usage_flush(shutdown: CancellationToken) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(usage::FLUSH_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = tick.tick() => flush_once().await,
                _ = shutdown.cancelled() => {
                    // What the last partial minute counted.
                    flush_once().await;
                    break;
                }
            }
        }
    })
}

/// One flush. Never touches the database on a quiet minute.
pub(crate) async fn flush_once() {
    if !usage::has_pending() {
        return;
    }
    let db = match establish_connection().await {
        Ok(db) => db,
        Err(e) => {
            tracing::warn!(error = %e, "token usage flush: DB connect failed; keeping counts");
            return;
        }
    };
    match usage::flush(&db).await {
        Ok(rows) => tracing::debug!(rows, "token usage flushed"),
        Err(e) => {
            tracing::warn!(error = %e, "token usage flush failed; counts kept for the next tick")
        }
    }
}
