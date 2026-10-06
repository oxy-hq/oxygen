//! The one sandbox audit row no request writes: an expiry's deletion.
//!
//! Every other sandbox row is built from the request's actor
//! (`AuditEntry::for_request`), so a key or token that created or deleted a
//! sandbox is named on it. The expiry sweep runs on the global worker's tick
//! for nobody, so its row names the system — and it is kept in this file on
//! its own, the only one here `call_sites_guard` allows a bare `AuditEntry::new`.

use oxy_app_core::audit::{ActorType, AuditEntry};

/// Who an expiry's audit row names.
const EXPIRY_ACTOR: &str = "system:sandbox-expiry";

/// An audit row for `action`, done by the expiry sweep.
pub(super) fn expiry_entry(action: &'static str) -> AuditEntry {
    let mut entry = AuditEntry::new(EXPIRY_ACTOR, action);
    entry.actor_type = ActorType::System;
    entry
}
