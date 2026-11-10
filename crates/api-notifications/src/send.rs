//! `oxy_app_core::notifications::send` plus the one thing it cannot do: deliver
//! inline.
//!
//! A send with no workspace has no run to queue the delivery under, so it is
//! delivered here, in the request. Delivery is the push, email and Slack
//! transport, which is this crate's; everything else is re-exported.

use sea_orm::DatabaseConnection;

pub use oxy_app_core::notifications::send::*;

/// Send one notification; deliver it inline when it has no workspace to queue
/// the delivery under. See [`oxy_app_core::notifications::send::send`] for the
/// order of writes.
pub async fn send(db: &DatabaseConnection, out: Outgoing) -> Result<Sent, SendError> {
    let inline = out.workspace_id.is_none();
    let sent = oxy_app_core::notifications::send::send(db, out).await?;
    if !sent.deduplicated && sent.recipients > 0 && inline {
        // Best-effort: the inbox row is written, and that is the contract.
        let report = super::delivery::deliver_send(db, sent.id).await;
        tracing::info!(send = %sent.id, ?report, "notification delivered inline");
    }
    Ok(sent)
}
