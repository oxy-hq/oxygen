//! [`TxnAuditSink`]: the one production implementation of the kernel port
//! [`entity::audit_sink::AuditSink`].
//!
//! A domain below this crate (`oxy-identity`'s token lifecycle) states each
//! row as a kernel [`AuditRow`]. The caller, which holds the connection and
//! knows who acted, builds the sink and hands it down; the sink adds the
//! actor and writes the row with [`record_in_txn`] on that connection. So the
//! row commits or rolls back with the change it records, exactly as when the
//! domain called `record_in_txn` itself.
//!
//! The actor is either the request's ([`AuditEntry::for_request`], which
//! stamps the key or token the request used) or the system's, for a row no
//! request wrote: a leak report, the token sweeper, the sandbox sweep. Only
//! the system half builds an entry with `AuditEntry::new`, which is why
//! `call_sites_guard` names this file; it also names every caller of
//! `TxnAuditSink::system`, so a request handler cannot write as the system.

use entity::audit_sink::{AuditRow, AuditSink, RecordFuture};
use sea_orm::ConnectionTrait;

use crate::audit::{ActorType, AuditContext, AuditEntry, RequestActor, record_in_txn};

/// `actor_email` on a system row.
const SYSTEM_ACTOR: &str = "system";

#[derive(Clone, Copy, Debug)]
enum Actor<'a> {
    Request(&'a RequestActor),
    System(&'a AuditContext),
}

/// Writes audit rows on `conn` (usually the caller's transaction), as one
/// actor.
pub struct TxnAuditSink<'a, C> {
    conn: &'a C,
    actor: Actor<'a>,
}

impl<'a, C: ConnectionTrait> TxnAuditSink<'a, C> {
    /// Rows a request wrote, as `actor`.
    pub fn request(conn: &'a C, actor: &'a RequestActor) -> Self {
        Self {
            conn,
            actor: Actor::Request(actor),
        }
    }

    /// Rows no request wrote, as the system, with `context` (the reporting
    /// client's address for a leak report; none for a sweep).
    pub fn system(conn: &'a C, context: &'a AuditContext) -> Self {
        Self {
            conn,
            actor: Actor::System(context),
        }
    }
}

impl<C> TxnAuditSink<'_, C> {
    /// The entry `row` is written as: the actor's entry for its action, with
    /// every field of the row on it. Private: the only way out of the sink is
    /// a write, so a system entry cannot be lifted out and written elsewhere.
    fn entry(&self, row: AuditRow) -> AuditEntry {
        let mut entry = match self.actor {
            Actor::Request(actor) => AuditEntry::for_request(actor, row.action),
            Actor::System(context) => system_entry(row.action, context),
        };
        let AuditRow {
            action: _,
            org_id,
            target_type,
            target_id,
            target_label,
            metadata,
            before,
            after,
        } = row;
        entry.org_id = org_id;
        entry.target_type = target_type;
        entry.target_id = target_id;
        entry.target_label = target_label;
        entry.metadata = metadata;
        entry.before = before;
        entry.after = after;
        entry
    }
}

impl<C: ConnectionTrait> AuditSink for TxnAuditSink<'_, C> {
    fn record(&self, row: AuditRow) -> RecordFuture<'_> {
        let entry = self.entry(row);
        let conn = self.conn;
        Box::pin(async move { record_in_txn(conn, entry).await.map(|_| ()) })
    }
}

/// An entry for `action` that the system wrote, with `context`.
fn system_entry(action: &'static str, context: &AuditContext) -> AuditEntry {
    let mut entry = AuditEntry::new(SYSTEM_ACTOR, action).context(context.clone());
    entry.actor_type = ActorType::System;
    entry
}

#[cfg(test)]
#[path = "sink_tests.rs"]
mod tests;
