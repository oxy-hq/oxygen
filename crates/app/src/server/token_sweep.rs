//! Periodic token hygiene, driven by the global singleton worker's tick
//! (`router::recovery`, `OXY_INPROC_GLOBAL_WORKER`) — beside the schedules and
//! the preview sweep, not in a loop of its own and never from a request.
//!
//! What a pass does, each step idempotent so any number of drivers can run it
//! at once, and each failing alone:
//!
//! - **spent OIDC `jti`s** past their `expires_at` are deleted. The replay
//!   ledger only has to outlive the token it guards, which expires in minutes;
//! - **expired `ci` token rows** more than 30 days past expiry are deleted. A
//!   trusted-access token lives 15 minutes and one is minted per CI job, so
//!   without this the table grows by a row per job, forever;
//! - **expiry notices** go out 7 days ahead, once per token per expiry —
//!   legacy keys included, since a mail only informs
//!   (`user_tokens::hygiene::send_expiry_notices`);
//! - **unused tokens** — new-format ones nobody used for a year — expire: the
//!   row stays, with `expires_at = now` (`expire_unused_tokens`).
//!
//! A legacy key is never deleted, expired or revoked by a sweep (design §3.5).
//!
//! The driver ticks every few seconds; this work is worth doing a few times an
//! hour. So [`sweep`] is throttled per process, and a tick inside the window
//! is one mutex read. The mail is bounded by a time budget, so a slow SES
//! never holds the driver's tick for long.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::Utc;
use oxy_auth::github_oidc::jti;
use oxy_auth::token::ci;
use sea_orm::DatabaseConnection;

use crate::server::api::user_tokens::hygiene::{self, NoticeReport};

/// The least time between two sweeps by one process.
const EVERY: Duration = Duration::from_secs(10 * 60);

static LAST_SWEPT: Mutex<Option<Instant>> = Mutex::new(None);

/// Whether a sweep is due, claiming it if so. A poisoned lock sweeps: the
/// deletes are idempotent, and skipping forever is the worse failure.
fn due(now: Instant) -> bool {
    let mut last = LAST_SWEPT.lock().unwrap_or_else(|e| e.into_inner());
    let due = last.is_none_or(|at| now.duration_since(at) >= EVERY);
    if due {
        *last = Some(now);
    }
    due
}

/// One pass, unthrottled. A failure of one step does not skip the others.
pub(crate) async fn run(db: &DatabaseConnection) {
    let now = Utc::now();
    match jti::sweep_expired(db, now).await {
        Ok(0) => {}
        Ok(swept) => tracing::info!(swept, "token sweep: spent OIDC jtis"),
        Err(e) => tracing::warn!(error = %e, "token sweep: the OIDC jti sweep failed"),
    }
    match ci::delete_expired(db, now).await {
        Ok(0) => {}
        Ok(swept) => tracing::info!(
            swept,
            keep_days = ci::KEEP_EXPIRED_DAYS,
            "token sweep: expired ci tokens"
        ),
        Err(e) => tracing::warn!(error = %e, "token sweep: the ci token sweep failed"),
    }
    match hygiene::expire_unused_tokens(db, now).await {
        Ok(0) => {}
        Ok(expired) => tracing::info!(expired, "token sweep: unused tokens expired"),
        Err(e) => tracing::warn!(error = ?e, "token sweep: the unused-token sweep failed"),
    }
    match hygiene::send_expiry_notices(db, now).await {
        Ok(report) if report == NoticeReport::default() => {}
        Ok(report) => tracing::info!(
            sent = report.sent,
            nobody = report.nobody,
            failed = report.failed,
            "token sweep: expiry notices"
        ),
        Err(e) => tracing::warn!(error = ?e, "token sweep: the expiry notices failed"),
    }
}

/// The global driver's hook: sweep if one is due, else return at once.
pub(crate) async fn sweep(db: &DatabaseConnection) {
    if due(Instant::now()) {
        run(db).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sweep_is_claimed_once_per_window() {
        // One process-wide throttle, so one test owns the whole sequence.
        let start = Instant::now();
        *LAST_SWEPT.lock().unwrap() = None;
        assert!(due(start), "the first tick sweeps");
        assert!(
            !due(start + Duration::from_secs(5)),
            "the next tick does not"
        );
        assert!(!due(start + EVERY - Duration::from_secs(1)));
        assert!(due(start + EVERY), "a tick past the window sweeps again");
        assert!(!due(start + EVERY + Duration::from_secs(5)));
    }
}
