//! Lifecycle rows that no request actor wrote (API-tokens design §3.7):
//!
//! - a **leak report** revoking a token — the route is public and
//!   unauthenticated, so there is no user and no credential behind it, only
//!   the reporting client's address;
//! - the **token sweeper** expiring an unused token — it runs on the global
//!   worker's tick, outside any request.
//!
//! Both write the same [`Event`] rows a request does — one per org the token
//! reaches, in the transaction that changes it — with `actor_type = system`.
//! This is the one place in the token code that builds an entry without
//! [`AuditEntry::for_request`]; `call_sites_guard` names it.

use oxy_app_core::audit::{self, ActorType, AuditContext, AuditEntry};
use sea_orm::ConnectionTrait;

use super::audit::Event;
use super::error::TokenError;

/// `actor_email` on a system row.
const SYSTEM_ACTOR: &str = "system";

impl Event<'_> {
    /// Write the event's rows as the system, with `context` (the reporting
    /// client's address for a leak report; none for the sweeper).
    pub(crate) async fn record_as_system<C: ConnectionTrait>(
        &self,
        txn: &C,
        context: &AuditContext,
    ) -> Result<(), TokenError> {
        let base = || {
            let mut entry = AuditEntry::new(SYSTEM_ACTOR, self.action).context(context.clone());
            entry.actor_type = ActorType::System;
            entry
        };
        for entry in self.entries_from(base) {
            audit::record_in_txn(txn, entry).await?;
        }
        Ok(())
    }
}
