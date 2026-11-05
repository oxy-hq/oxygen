//! A route call whose caller hangs up before the function answers.
//!
//! A route invocation runs inline in the HTTP handler's future, and the row
//! is settled by the code *after* the isolate returns. When the browser goes
//! away (a page reload, a navigation, an aborted `fetch`) hyper drops that
//! future, so none of that code runs: the row stayed `running` until the lazy
//! reaper flipped it to `timeout` fifteen minutes later — fingerprinted as a
//! failure, and paged as "reaped: instance died or detached" for what was a
//! user leaving a page. Pages that fan out several slow functions on load made
//! this the dominant "failure" for some apps.
//!
//! [`ClientGone`] is armed before the run and disarmed once the row is
//! settled. Dropped while armed, it cancels the run's token (which stops the
//! cancel watchdog) and settles the row as `cancelled` — the existing status a
//! cancellation records, which is not a failure (`Failure::of` returns
//! `None`) and so neither pages nor dents availability.
//!
//! **Except a keyed call.** An `Idempotency-Key` exists for exactly this case:
//! the connection drops and the client retries with the same key. A keyed row
//! settled `cancelled` would be reclaimed and the function run again
//! (`resolve_keyed_row` replays only `success`), duplicating whatever the first
//! run had already written. A keyed row is therefore left `running`, so a retry
//! is still answered `409 IdempotencyKeyInProgress` until the reaper closes it
//! — the behaviour keyed calls always had. The token still fires.

use std::time::Instant;

use entity::app_function_invocations;
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// What the row says when the caller left. Names the cause plainly, so an
/// operator reading one row doesn't go looking for a dead instance.
pub(super) const CLIENT_GONE_ERROR: &str = "client disconnected before the function finished";

/// Settles a route invocation as `cancelled` if its handler future is dropped
/// before [`ClientGone::disarm`].
pub(super) struct ClientGone {
    db: DatabaseConnection,
    invocation_id: Uuid,
    started: Instant,
    token: CancellationToken,
    /// The call carried an `Idempotency-Key`: leave the row `running`.
    keyed: bool,
    armed: bool,
}

impl ClientGone {
    pub(super) fn arm(
        db: &DatabaseConnection,
        invocation_id: Uuid,
        started: Instant,
        keyed: bool,
    ) -> Self {
        Self {
            db: db.clone(),
            invocation_id,
            started,
            token: CancellationToken::new(),
            keyed,
            armed: true,
        }
    }

    /// The run's cancel token: fired when the caller leaves.
    pub(super) fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    /// The row is settled; a drop from here on is the normal end of the call.
    pub(super) fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for ClientGone {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.token.cancel();
        if self.keyed {
            tracing::info!(
                target: "oxy.custom_app.invocation",
                invocation_id = %self.invocation_id,
                "keyed route caller disconnected mid-run; row left running so a retry is refused, not re-run"
            );
            return;
        }
        let duration_ms = self.started.elapsed().as_millis() as i64;
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => {
                rt.spawn(settle_abandoned(
                    self.db.clone(),
                    self.invocation_id,
                    duration_ms,
                ));
            }
            // Only reachable if the runtime itself is shutting down; the
            // reaper still closes the row, just as a timeout.
            Err(_) => tracing::warn!(
                invocation_id = %self.invocation_id,
                "route caller left during runtime shutdown; leaving the row to the reaper"
            ),
        }
    }
}

/// Settle an abandoned invocation as `cancelled`, only if it is still
/// `running` — a row the run already settled (it finished just as the caller
/// left) keeps its real outcome.
pub(super) async fn settle_abandoned(
    db: DatabaseConnection,
    invocation_id: Uuid,
    duration_ms: i64,
) {
    let settled = app_function_invocations::Entity::update_many()
        .col_expr(
            app_function_invocations::Column::Status,
            Expr::value("cancelled"),
        )
        .col_expr(
            app_function_invocations::Column::Error,
            Expr::value(CLIENT_GONE_ERROR),
        )
        .col_expr(
            app_function_invocations::Column::DurationMs,
            Expr::value(Some(duration_ms)),
        )
        .filter(app_function_invocations::Column::Id.eq(invocation_id))
        .filter(app_function_invocations::Column::Status.eq("running"))
        .exec(&db)
        .await;
    match settled {
        Ok(r) if r.rows_affected > 0 => tracing::info!(
            target: "oxy.custom_app.invocation",
            %invocation_id,
            duration_ms,
            "route caller disconnected mid-run; invocation settled as cancelled"
        ),
        Ok(_) => {}
        Err(e) => tracing::warn!(%invocation_id, "settling an abandoned invocation failed: {e}"),
    }
}

#[cfg(all(test, feature = "custom-app-functions"))]
mod tests {
    use std::time::Duration;

    /// The watchdog exits once its run is gone. A route run dropped by a
    /// hung-up caller never reaches `watchdog.abort()`, and the watchdog used
    /// to poll its row once a second for the life of the process. The
    /// connection is disconnected (`default()`): an exit that waited for a poll would
    /// error instead, and the arm under test answers before the first poll.
    #[tokio::test]
    async fn the_cancel_watchdog_exits_when_its_run_is_dropped() {
        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
        drop(cancel_rx);
        let watchdog = super::super::spawn_cancel_watchdog(
            sea_orm::DatabaseConnection::default(),
            uuid::Uuid::new_v4(),
            cancel_tx,
            tokio_util::sync::CancellationToken::new(),
        );
        tokio::time::timeout(Duration::from_millis(500), watchdog)
            .await
            .expect("a watchdog whose run is gone returns at once, before its first poll");
    }
}
