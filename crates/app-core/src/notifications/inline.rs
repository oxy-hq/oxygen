//! Delivering a send inline, in the request, when it has no workspace to queue
//! the `notification_deliver` task under (an org with no workspace yet, a
//! platform notice raised before onboarding finishes).
//!
//! Delivery is the push, email and Slack transport, which is the notifications
//! surface's (`oxy-api-notifications`); the write path that needs it is here,
//! below every source. So the transport is handed in: `oxy-server` passes it
//! through `SurfaceSeams::notifications`, and `oxy serve` and `oxy worker`
//! install it at boot and **refuse to start without it**
//! (`SurfaceSeams::require_notifications`), so a process that raises notices
//! cannot silently skip their delivery (the fail-silent inversion
//! `internal-docs/domain-boundaries.md` warns about).
//!
//! A process that never installs it — a one-shot CLI command, a test that did
//! not ask for delivery — still writes the inbox row, which is the contract;
//! the skipped delivery is logged at `warn`.

use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;

use sea_orm::DatabaseConnection;
use uuid::Uuid;

/// Delivers one send now and describes what happened, for the caller's log.
pub type InlineDeliver =
    for<'a> fn(&'a DatabaseConnection, Uuid) -> Pin<Box<dyn Future<Output = String> + Send + 'a>>;

static DELIVER: OnceLock<InlineDeliver> = OnceLock::new();

/// Install the transport for this process. First install wins; the
/// composition root hands in one function, so a second is the same one.
pub fn install(deliver: InlineDeliver) {
    let _ = DELIVER.set(deliver);
}

/// Whether [`install`] has run in this process.
pub fn installed() -> bool {
    DELIVER.get().is_some()
}

/// Deliver `send_id` now, best-effort. Without an installed transport the
/// inbox row stands alone and this says so.
pub async fn deliver(db: &DatabaseConnection, send_id: Uuid) -> String {
    match DELIVER.get() {
        Some(deliver) => deliver(db, send_id).await,
        None => {
            tracing::warn!(
                send = %send_id,
                "no notification transport installed in this process: the inbox row is written, \
                 push, email and Slack are not attempted"
            );
            "not delivered: no transport installed".to_string()
        }
    }
}
