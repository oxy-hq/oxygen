//! The OLTP change stream, on the platform side (design
//! `2026-11-22-oltp-change-stream-design.md`, §5.2): every committed change to
//! a table an app declares under `oltpChanges` lands in its store's
//! `oxy_changes.log`, in the same transaction, with who made it.
//!
//! - [`install`]: capture, installed at a production promote after the app's
//!   OLTP migrations and before its pointer moves, on the tables the build
//!   declares, and removed from those it no longer does. Behind the per-org
//!   flag `oltp_changes` ([`enabled_for`]).
//! - [`provenance`]: who made each change, signed. The host signs every
//!   production `ctx.oltp` transaction of an app that declares a table, and
//!   each migration file such an app applies to production; the relay
//!   verifies the MAC each log row carries.
//! - [`relay`]: the log, shipped to typed change tables in the app's
//!   workspace Airhouse (§5.3, §5.4).

use uuid::Uuid;

pub mod install;
mod owner;
pub mod provenance;
pub mod relay;

/// Whether `oltp_changes` is on for `org_id`. Off, a production promote
/// installs and removes no capture and opens no store connection for it,
/// whatever its build declares: a publish does what it did before the flag.
pub fn enabled_for(org_id: Uuid) -> bool {
    use oxy_feature_flags::{FlagContext, OLTP_CHANGES, is_enabled_for};
    is_enabled_for(OLTP_CHANGES, &FlagContext::org(org_id))
}

/// Whether a build's manifest declares a table to capture: an
/// `oltpChanges.tables` object with at least one entry. Read from the raw
/// manifest JSON the build stores, which the publish already checked; any
/// other shape declares nothing.
pub fn declares_tables(manifest: Option<&serde_json::Value>) -> bool {
    manifest
        .and_then(|m| m.get("oltpChanges"))
        .and_then(|changes| changes.get("tables"))
        .and_then(serde_json::Value::as_object)
        .is_some_and(|tables| !tables.is_empty())
}

/// Whether a build's writes are signed for the change stream: the host signs
/// each production `ctx.oltp` transaction, and a promote each migration file,
/// of a build that declares a table.
///
/// Deliberately not gated on `oltp_changes`. The flag decides what a promote
/// installs, but turning it off removes nothing: capture already installed
/// keeps writing log rows, and a row written unsigned ships as `unverified`
/// with its actor lost. So signing follows what may be installed, and a build
/// that declares tables in an org that never had the flag pays two round
/// trips per transaction for settings nothing reads. Only an app that put
/// `oltpChanges` in its own manifest pays that.
pub fn signs(manifest: Option<&serde_json::Value>) -> bool {
    declares_tables(manifest)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::declares_tables;

    #[test]
    fn only_a_non_empty_tables_object_declares_a_table() {
        let declared = json!({ "oltpChanges": { "tables": { "orders": {} } } });
        assert!(declares_tables(Some(&declared)));
        for manifest in [
            json!({}),
            json!({ "oltpChanges": {} }),
            json!({ "oltpChanges": { "tables": {} } }),
            json!({ "oltpChanges": { "tables": ["orders"] } }),
            json!({ "oltpChanges": true }),
        ] {
            assert!(!declares_tables(Some(&manifest)), "{manifest}");
        }
        assert!(!declares_tables(None));
    }
}
