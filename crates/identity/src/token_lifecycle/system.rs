//! The token lifecycle rows no request wrote (API-tokens design §3.7): a leak
//! report revoking a token (public, unauthenticated: only the reporting
//! client's address), the token sweeper expiring an unused token and the
//! sandbox sweep queuing an ended sandbox agent token's teardowns (both on the
//! global worker's tick). They write the same rows a request does, with
//! `actor_type = system`.
//!
//! The one file of the token audit that builds an entry without
//! `AuditEntry::for_request`; `oxy-app-core`'s `call_sites_guard` names it, so
//! keep the request-written path out of it.

use oxy_app_core::audit::{ActorType, AuditContext, AuditEntry, record_in_txn};
use sea_orm::{ConnectionTrait, DbErr};

use super::Event;

/// `actor_email` on a system row.
const SYSTEM_ACTOR: &str = "system";

impl Event<'_> {
    /// Write the event's rows as the system, with `context` (the reporting
    /// client's address for a leak report; none for the sweeper).
    pub async fn record_as_system<C: ConnectionTrait>(
        &self,
        txn: &C,
        context: &AuditContext,
    ) -> Result<(), DbErr> {
        for entry in self.entries_from(|| system_entry(self.action, context)) {
            record_in_txn(txn, entry).await?;
        }
        Ok(())
    }
}

/// An entry for `action` that the system wrote, with `context`. Also what
/// [`super::sandboxes_queued`] starts its per-org rows from.
pub fn system_entry(action: &'static str, context: &AuditContext) -> AuditEntry {
    let mut entry = AuditEntry::new(SYSTEM_ACTOR, action).context(context.clone());
    entry.actor_type = ActorType::System;
    entry
}
